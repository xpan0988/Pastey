//! Deterministic requester-side orchestration for authored native-v2 Plans.
//!
//! This module is a product/backend coordinator, not a planner. It accepts only
//! explicit HostRefs, roots, and Search/Transform/Transfer/Execute steps, seals
//! the existing v2 semantic representation, and coordinates that immutable
//! revision. It never inserts a step, changes topology, or turns transport
//! availability into managed effect authority.

#![allow(dead_code)] // The service is intentionally broader than the first UI command seam.

use std::{collections::BTreeMap, sync::Arc};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    bridge_plan::StepOperation,
    bridge_plan_v2::{
        participant_for_ref, seal_revision, verify_sealed_revision, AttemptStartV2,
        ManagedObjectRevisionV2, PlanApprovalV2, PlanRevisionV2, PlanRootV2, PlanStepV2,
        ReviewRequestV2, PROTOCOL_VERSION,
    },
    error::{AppError, AppResult},
    host_identity::{HostRef, PlanParticipantRef, PlanParticipants},
    host_runtime::HostRuntime,
    storage::AppPaths,
};

pub(crate) const PRODUCT_SCHEMA_VERSION: &str = "pastey-native-v2-product-v1";
pub(crate) const PRODUCT_STATUS_EVENT: &str = "pastey://native-v2-plan-status";
pub(crate) const READINESS_REQUEST_KIND: &str = "bridge_plan.v2.readiness_request";
pub(crate) const READINESS_KIND: &str = "bridge_plan.v2.readiness_result";
pub(crate) const PREPARED_KIND: &str = "bridge_plan.v2.attempt_prepared";
pub(crate) const COMMIT_KIND: &str = "bridge_plan.v2.attempt_commit";
pub(crate) const STEP_RESULT_KIND: &str = "bridge_plan.v2.step_result";
pub(crate) const STEP_FAILURE_KIND: &str = "bridge_plan.v2.step_failure";
pub(crate) const STEP_COMMIT_KIND: &str = "bridge_plan.v2.step_commit";
pub(crate) const CANCEL_KIND: &str = "bridge_plan.v2.attempt_cancel";

const MAX_LIFETIME_SECONDS: i64 = 24 * 60 * 60;
const MAX_ID: usize = 128;
const MAX_TEXT: usize = 1_024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeV2RootDraftV1 {
    pub root_id: String,
    pub object: NativeV2ObjectRevisionDtoV1,
    pub host_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeV2ObjectRevisionDtoV1 {
    pub logical_object_id: String,
    pub revision: u64,
}

impl From<&ManagedObjectRevisionV2> for NativeV2ObjectRevisionDtoV1 {
    fn from(value: &ManagedObjectRevisionV2) -> Self {
        Self {
            logical_object_id: value.logical_object_id.clone(),
            revision: value.revision,
        }
    }
}

impl From<NativeV2ObjectRevisionDtoV1> for ManagedObjectRevisionV2 {
    fn from(value: NativeV2ObjectRevisionDtoV1) -> Self {
        Self {
            logical_object_id: value.logical_object_id,
            revision: value.revision,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeV2StepDraftV1 {
    Search {
        step_id: String,
        depends_on: Vec<String>,
        host_ref: String,
        output: NativeV2ObjectRevisionDtoV1,
        query: String,
        safe_scope_labels: Vec<String>,
    },
    Transform {
        step_id: String,
        depends_on: Vec<String>,
        host_ref: String,
        input: NativeV2ObjectRevisionDtoV1,
        output: NativeV2ObjectRevisionDtoV1,
        modification_intent: String,
    },
    Transfer {
        step_id: String,
        depends_on: Vec<String>,
        source_host_ref: String,
        destination_host_ref: String,
        input: NativeV2ObjectRevisionDtoV1,
        output: NativeV2ObjectRevisionDtoV1,
    },
    Execute {
        step_id: String,
        depends_on: Vec<String>,
        host_ref: String,
        target: NativeV2ObjectRevisionDtoV1,
        execution_intent: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeV2ComposeRequestV1 {
    pub plan_id: String,
    pub revision_id: String,
    pub revision_number: u32,
    pub bridge_id: String,
    pub requester_host_ref: String,
    pub participant_host_refs: Vec<String>,
    pub roots: Vec<NativeV2RootDraftV1>,
    pub original_user_goal: String,
    pub expected_outcome: String,
    pub steps: Vec<NativeV2StepDraftV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeV2ProductStateV1 {
    Draft,
    Approved,
    CheckingReadiness,
    Preparing,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl NativeV2ProductStateV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::CheckingReadiness => "checking_readiness",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeV2PlanStatusV1 {
    pub schema_version: String,
    pub plan_id: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub approval_id: Option<String>,
    pub attempt_id: Option<String>,
    pub state: NativeV2ProductStateV1,
    pub current_step_id: Option<String>,
    pub completed_steps: u32,
    pub total_steps: u32,
    pub ready_hosts: u32,
    pub total_hosts: u32,
    pub code: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2ReadinessV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) correlation_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) participant: PlanParticipantRef,
    pub(crate) host_ref: HostRef,
    pub(crate) session_binding_ref: String,
    pub(crate) ready: bool,
    pub(crate) code: Option<String>,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2ReadinessRequestV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) correlation_id: String,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2PreparedV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) correlation_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) participant: PlanParticipantRef,
    pub(crate) host_ref: HostRef,
    pub(crate) admission_ref: String,
    pub(crate) session_binding_ref: String,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2AttemptCommitV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2StepResultV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) step_id: String,
    pub(crate) operation: StepOperation,
    pub(crate) participant: PlanParticipantRef,
    pub(crate) host_ref: HostRef,
    pub(crate) object: Option<ManagedObjectRevisionV2>,
    pub(crate) content_digest: Option<String>,
    pub(crate) result_digest: Option<String>,
    pub(crate) session_binding_ref: String,
    pub(crate) completion_ref: String,
    pub(crate) completed_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2StepCommitV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) sender: PlanParticipantRef,
    pub(crate) target: PlanParticipantRef,
    pub(crate) result: NativeV2StepResultV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2StepFailureV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) step_id: Option<String>,
    pub(crate) participant: PlanParticipantRef,
    pub(crate) host_ref: HostRef,
    pub(crate) session_binding_ref: String,
    pub(crate) code: String,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2AttemptCancelV1 {
    pub(crate) protocol_version: String,
    pub(crate) message_id: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) sender: PlanParticipantRef,
    pub(crate) target: PlanParticipantRef,
    pub(crate) reason_code: String,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeV2TransferMetadataV1 {
    pub(crate) protocol_version: String,
    pub(crate) attempt_id: String,
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) step_id: String,
    pub(crate) source: PlanParticipantRef,
    pub(crate) destination: PlanParticipantRef,
    pub(crate) source_host_ref: HostRef,
    pub(crate) destination_host_ref: HostRef,
    pub(crate) object: ManagedObjectRevisionV2,
    pub(crate) content_digest: String,
    pub(crate) expires_at: i64,
}

pub(crate) fn compose_revision(request: NativeV2ComposeRequestV1) -> AppResult<PlanRevisionV2> {
    id(&request.plan_id, "plan id")?;
    id(&request.revision_id, "revision id")?;
    id(&request.bridge_id, "Bridge id")?;
    text(&request.original_user_goal, "user goal")?;
    text(&request.expected_outcome, "expected outcome")?;
    let requester_host = HostRef::parse(request.requester_host_ref)?;
    let mut hosts = request
        .participant_host_refs
        .into_iter()
        .map(HostRef::parse)
        .collect::<AppResult<Vec<_>>>()?;
    if !hosts.contains(&requester_host) {
        hosts.push(requester_host.clone());
    }
    let participants = PlanParticipants::new(&request.plan_id, hosts)?;
    let by_host = participants
        .as_slice()
        .iter()
        .map(|participant| {
            (
                participant.host_ref.clone(),
                participant.participant_ref.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let requester = participant(&by_host, &requester_host)?;
    let roots = request
        .roots
        .into_iter()
        .map(|root| {
            let host = HostRef::parse(root.host_ref)?;
            Ok(PlanRootV2 {
                root_id: root.root_id,
                object: root.object.into(),
                host: participant(&by_host, &host)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let steps = request
        .steps
        .into_iter()
        .map(|step| lower_step(step, &by_host))
        .collect::<AppResult<Vec<_>>>()?;
    seal_revision(PlanRevisionV2 {
        schema_version: crate::bridge_plan_v2::PLAN_SCHEMA_VERSION.into(),
        plan_id: request.plan_id,
        revision_id: request.revision_id,
        revision_number: request.revision_number,
        revision_hash: String::new(),
        bridge_id: request.bridge_id,
        requester,
        participants,
        roots,
        original_user_goal: request.original_user_goal,
        expected_outcome: request.expected_outcome,
        steps,
    })
}

pub(crate) fn protocol_replay_id(
    kind: &str,
    payload: Value,
    expected_bridge: &str,
    now: i64,
) -> AppResult<String> {
    let (message_id, bridge_id, expires_at) = match kind {
        READINESS_REQUEST_KIND => {
            let value: NativeV2ReadinessRequestV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        READINESS_KIND => {
            let value: NativeV2ReadinessV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        PREPARED_KIND => {
            let value: NativeV2PreparedV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        COMMIT_KIND => {
            let value: NativeV2AttemptCommitV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        STEP_RESULT_KIND => {
            let value: NativeV2StepResultV1 = serde_json::from_value(payload)?;
            (
                value.message_id,
                value.bridge_id,
                value.completed_at + MAX_LIFETIME_SECONDS,
            )
        }
        STEP_FAILURE_KIND => {
            let value: NativeV2StepFailureV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        STEP_COMMIT_KIND => {
            let value: NativeV2StepCommitV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        CANCEL_KIND => {
            let value: NativeV2AttemptCancelV1 = serde_json::from_value(payload)?;
            (value.message_id, value.bridge_id, value.expires_at)
        }
        _ => return invalid("Unsupported native v2 product protocol event."),
    };
    id(&message_id, "message id")?;
    if bridge_id != expected_bridge || expires_at <= now {
        return invalid("Native v2 product event Bridge or lifetime is invalid.");
    }
    Ok(format!("native-v2:{kind}:{message_id}"))
}

fn lower_step(
    step: NativeV2StepDraftV1,
    participants: &BTreeMap<HostRef, PlanParticipantRef>,
) -> AppResult<PlanStepV2> {
    Ok(match step {
        NativeV2StepDraftV1::Search {
            step_id,
            depends_on,
            host_ref,
            output,
            query,
            safe_scope_labels,
        } => {
            if query.len() > 128 {
                return invalid(
                    "Native v2 Search query exceeds the deterministic filename adapter.",
                );
            }
            let host = HostRef::parse(host_ref)?;
            PlanStepV2::Search {
                step_id,
                depends_on,
                host: participant(participants, &host)?,
                output: output.into(),
                query,
                safe_scope_labels,
            }
        }
        NativeV2StepDraftV1::Transform {
            step_id,
            depends_on,
            host_ref,
            input,
            output,
            modification_intent,
        } => {
            let host = HostRef::parse(host_ref)?;
            PlanStepV2::Transform {
                step_id,
                depends_on,
                host: participant(participants, &host)?,
                input: input.into(),
                output: output.into(),
                modification_intent,
            }
        }
        NativeV2StepDraftV1::Transfer {
            step_id,
            depends_on,
            source_host_ref,
            destination_host_ref,
            input,
            output,
        } => {
            let source = HostRef::parse(source_host_ref)?;
            let destination = HostRef::parse(destination_host_ref)?;
            PlanStepV2::Transfer {
                step_id,
                depends_on,
                source: participant(participants, &source)?,
                destination: participant(participants, &destination)?,
                input: input.into(),
                output: output.into(),
            }
        }
        NativeV2StepDraftV1::Execute {
            step_id,
            depends_on,
            host_ref,
            target,
            execution_intent,
        } => {
            let host = HostRef::parse(host_ref)?;
            PlanStepV2::Execute {
                step_id,
                depends_on,
                host: participant(participants, &host)?,
                target: target.into(),
                execution_intent,
            }
        }
    })
}

fn participant(
    participants: &BTreeMap<HostRef, PlanParticipantRef>,
    host: &HostRef,
) -> AppResult<PlanParticipantRef> {
    participants
        .get(host)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Step Host is not a Plan participant.".into()))
}

pub(crate) struct NativeV2ProductStore<'a> {
    paths: &'a AppPaths,
}

impl<'a> NativeV2ProductStore<'a> {
    pub(crate) fn new(paths: &'a AppPaths) -> Self {
        Self { paths }
    }

    pub(crate) fn create_draft(
        &self,
        revision: &PlanRevisionV2,
        local_host_ref: &HostRef,
        now: i64,
    ) -> AppResult<NativeV2PlanStatusV1> {
        verify_sealed_revision(revision)?;
        if crate::bridge_plan_v2::requester_host(revision)? != local_host_ref {
            return invalid("Only the authored requester Host may create this product Plan.");
        }
        ensure_active_bridge(self.paths, &revision.bridge_id)?;
        let mut conn = connection(self.paths)?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO native_v2_product_revisions
             (revision_id, plan_id, bridge_id, revision_hash, state, revision_json,
              failure_code, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, NULL, ?6, ?6)",
            params![
                revision.revision_id,
                revision.plan_id,
                revision.bridge_id,
                revision.revision_hash,
                serde_json::to_string(revision)?,
                now
            ],
        )?;
        for step in &revision.steps {
            tx.execute(
                "INSERT INTO native_v2_product_steps
                 (revision_id, attempt_id, step_id, operation, state, completion_ref, updated_at)
                 VALUES (?1, NULL, ?2, ?3, 'pending', NULL, ?4)",
                params![revision.revision_id, step.id(), operation_name(step), now],
            )?;
        }
        tx.commit()?;
        self.status_for_revision(&revision.revision_id)
    }

    pub(crate) fn approve(
        &self,
        revision_id: &str,
        approval_id: &str,
        expires_at: i64,
        now: i64,
    ) -> AppResult<PlanApprovalV2> {
        id(approval_id, "approval id")?;
        if expires_at <= now || expires_at > now + MAX_LIFETIME_SECONDS {
            return invalid("Native v2 approval expiry is invalid.");
        }
        let revision = self.revision(revision_id)?;
        let approval = PlanApprovalV2 {
            approval_id: approval_id.into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            requester: revision.requester.clone(),
            expires_at,
        };
        let mut conn = connection(self.paths)?;
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE native_v2_product_revisions SET state = 'approved', updated_at = ?2
             WHERE revision_id = ?1 AND state = 'draft'",
            params![revision_id, now],
        )?;
        if changed != 1 {
            return invalid("Native v2 revision is not available for approval.");
        }
        tx.execute(
            "INSERT INTO native_v2_product_approvals
             (approval_id, revision_id, revision_hash, expires_at, state, approval_json, created_at)
             VALUES (?1, ?2, ?3, ?4, 'valid', ?5, ?6)",
            params![
                approval.approval_id,
                approval.revision_id,
                approval.revision_hash,
                approval.expires_at,
                serde_json::to_string(&approval)?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(approval)
    }

    pub(crate) fn revision(&self, revision_id: &str) -> AppResult<PlanRevisionV2> {
        let json = connection(self.paths)?
            .query_row(
                "SELECT revision_json FROM native_v2_product_revisions WHERE revision_id = ?1",
                [revision_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                AppError::NotFound("Native v2 product revision is unavailable.".into())
            })?;
        let revision = serde_json::from_str(&json)?;
        verify_sealed_revision(&revision)?;
        Ok(revision)
    }

    pub(crate) fn approval(&self, approval_id: &str, now: i64) -> AppResult<PlanApprovalV2> {
        let stored = connection(self.paths)?
            .query_row(
                "SELECT state, expires_at, approval_json FROM native_v2_product_approvals
                 WHERE approval_id = ?1",
                [approval_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound("Native v2 approval is unavailable.".into()))?;
        if stored.0 != "valid" || stored.1 <= now {
            return invalid("Native v2 approval is no longer valid.");
        }
        Ok(serde_json::from_str(&stored.2)?)
    }

    pub(crate) fn status_for_revision(&self, revision_id: &str) -> AppResult<NativeV2PlanStatusV1> {
        let conn = connection(self.paths)?;
        let row = conn
            .query_row(
                "SELECT r.plan_id, r.revision_hash, r.state, r.failure_code, r.updated_at,
                        a.approval_id,
                        t.attempt_id
                 FROM native_v2_product_revisions r
                 LEFT JOIN native_v2_product_approvals a ON a.revision_id = r.revision_id
                 LEFT JOIN native_v2_product_attempts t ON t.revision_id = r.revision_id
                 WHERE r.revision_id = ?1 ORDER BY t.created_at DESC LIMIT 1",
                [revision_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound("Native v2 product Plan is unavailable.".into()))?;
        let attempt_id = row.6;
        let (completed_steps, total_steps, current_step_id) = if let Some(attempt_id) = &attempt_id
        {
            let completed: u32 = conn.query_row(
                "SELECT COUNT(*) FROM native_v2_product_steps
                 WHERE attempt_id = ?1 AND state = 'completed'",
                [attempt_id],
                |value| value.get(0),
            )?;
            let total: u32 = conn.query_row(
                "SELECT COUNT(*) FROM native_v2_product_steps WHERE attempt_id = ?1",
                [attempt_id],
                |value| value.get(0),
            )?;
            let current = conn
                .query_row(
                    "SELECT step_id FROM native_v2_product_steps
                     WHERE attempt_id = ?1 AND state IN ('eligible','running')
                     ORDER BY rowid LIMIT 1",
                    [attempt_id],
                    |value| value.get::<_, String>(0),
                )
                .optional()?;
            (completed, total, current)
        } else {
            let total = conn.query_row(
                "SELECT COUNT(*) FROM native_v2_product_steps
                 WHERE revision_id = ?1 AND attempt_id IS NULL",
                [revision_id],
                |value| value.get(0),
            )?;
            (0, total, None)
        };
        let (ready_hosts, total_hosts) = if let Some(attempt_id) = &attempt_id {
            conn.query_row(
                "SELECT SUM(CASE WHEN readiness_state = 'ready' THEN 1 ELSE 0 END), COUNT(*)
                 FROM native_v2_product_hosts WHERE attempt_id = ?1",
                [attempt_id],
                |value| {
                    Ok((
                        value.get::<_, Option<u32>>(0)?.unwrap_or(0),
                        value.get::<_, u32>(1)?,
                    ))
                },
            )?
        } else {
            (0, 0)
        };
        Ok(NativeV2PlanStatusV1 {
            schema_version: PRODUCT_SCHEMA_VERSION.into(),
            plan_id: row.0,
            revision_id: revision_id.into(),
            revision_hash: row.1,
            approval_id: row.5,
            attempt_id,
            state: parse_product_state(&row.2)?,
            current_step_id,
            completed_steps,
            total_steps,
            ready_hosts,
            total_hosts,
            code: row.3,
            updated_at: row.4,
        })
    }
}

#[derive(Clone)]
pub(crate) struct NativeV2OutboundEventV1 {
    pub(crate) room_id: String,
    pub(crate) peer_route_ref: String,
    pub(crate) event: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalReadinessV1 {
    ready: bool,
    code: Option<&'static str>,
}

impl HostRuntime {
    pub(crate) fn compose_native_v2_product_plan(
        &self,
        request: NativeV2ComposeRequestV1,
        now: i64,
    ) -> AppResult<NativeV2PlanStatusV1> {
        let revision = compose_revision(request)?;
        let status = NativeV2ProductStore::new(&self.paths).create_draft(
            &revision,
            &self.local_host_ref,
            now,
        )?;
        self.emit(PRODUCT_STATUS_EVENT, &status)?;
        Ok(status)
    }

    pub(crate) fn approve_native_v2_product_plan(
        &self,
        revision_id: &str,
        approval_id: &str,
        expires_at: i64,
        now: i64,
    ) -> AppResult<NativeV2PlanStatusV1> {
        NativeV2ProductStore::new(&self.paths).approve(
            revision_id,
            approval_id,
            expires_at,
            now,
        )?;
        let status = NativeV2ProductStore::new(&self.paths).status_for_revision(revision_id)?;
        self.emit(PRODUCT_STATUS_EVENT, &status)?;
        Ok(status)
    }

    pub(crate) async fn start_native_v2_product_attempt(
        self: &Arc<Self>,
        approval_id: &str,
        attempt_id: &str,
        expires_at: i64,
        now: i64,
    ) -> AppResult<NativeV2PlanStatusV1> {
        let events = prepare_requester_attempt(self, approval_id, attempt_id, expires_at, now)?;
        let revision_id = NativeV2ProductStore::new(&self.paths)
            .approval(approval_id, now)?
            .revision_id;
        let initial = NativeV2ProductStore::new(&self.paths).status_for_revision(&revision_id)?;
        self.emit(PRODUCT_STATUS_EVENT, &initial)?;
        for event in events {
            if let Err(error) = send_native_v2_event(self.clone(), event).await {
                let cancellation = terminate_requester_attempt(
                    self,
                    attempt_id,
                    "interrupted",
                    "review_delivery_failed",
                    crate::storage::now_ts(),
                )?;
                for event in cancellation {
                    let _ = send_native_v2_event(self.clone(), event).await;
                }
                let failed =
                    NativeV2ProductStore::new(&self.paths).status_for_revision(&revision_id)?;
                let _ = self.emit(PRODUCT_STATUS_EVENT, &failed);
                return Err(error);
            }
        }
        NativeV2ProductStore::new(&self.paths).status_for_revision(&revision_id)
    }

    pub(crate) fn native_v2_product_status(
        &self,
        revision_id: &str,
    ) -> AppResult<NativeV2PlanStatusV1> {
        NativeV2ProductStore::new(&self.paths).status_for_revision(revision_id)
    }

    pub(crate) async fn cancel_native_v2_product_attempt(
        self: &Arc<Self>,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<NativeV2PlanStatusV1> {
        let events =
            terminate_requester_attempt(self, attempt_id, "cancelled", "user_cancelled", now)?;
        for event in events {
            let _ = send_native_v2_event(self.clone(), event).await;
        }
        let revision_id: String = connection(&self.paths)?.query_row(
            "SELECT revision_id FROM native_v2_product_attempts WHERE attempt_id = ?1",
            [attempt_id],
            |row| row.get(0),
        )?;
        let status = NativeV2ProductStore::new(&self.paths).status_for_revision(&revision_id)?;
        self.emit(PRODUCT_STATUS_EVENT, &status)?;
        Ok(status)
    }

    fn native_v2_local_readiness(
        &self,
        revision: &PlanRevisionV2,
        target: &PlanParticipantRef,
        now: i64,
    ) -> AppResult<LocalReadinessV1> {
        let participant = participant_for_ref(revision, target)
            .ok_or_else(|| AppError::InvalidInput("Native v2 target is unavailable.".into()))?;
        if participant.host_ref != self.local_host_ref {
            return invalid("Native v2 readiness target does not match this Host.");
        }
        // The requester currently coordinates remote receiver attempts but does
        // not create a receiver admission for itself.  Treat every authored
        // requester-local primitive as unavailable until that distinct
        // self-admission/execution path exists; otherwise Search or Transfer
        // could be projected ready and then wait forever after commit.
        if requester_has_local_step(revision, target, &self.local_host_ref) {
            return Ok(LocalReadinessV1 {
                ready: false,
                code: Some("requester_self_execution_unavailable"),
            });
        }
        for root in &revision.roots {
            if &root.host == target
                && self
                    .managed_objects
                    .lock()
                    .acquisition_for_revision(
                        &revision.bridge_id,
                        &root.object.logical_object_id,
                        root.object.revision,
                        now,
                    )
                    .is_err()
            {
                return Ok(LocalReadinessV1 {
                    ready: false,
                    code: Some("managed_root_unavailable"),
                });
            }
        }
        for step in &revision.steps {
            match step {
                PlanStepV2::Transfer {
                    source,
                    destination,
                    ..
                } if source == target || destination == target => {
                    let counterpart = if source == target {
                        destination
                    } else {
                        source
                    };
                    let counterpart =
                        participant_for_ref(revision, counterpart).ok_or_else(|| {
                            AppError::InvalidInput(
                                "Native v2 Transfer participant is unavailable.".into(),
                            )
                        })?;
                    if peer_binding_for_host(self, &revision.bridge_id, &counterpart.host_ref, now)
                        .is_err()
                    {
                        return Ok(LocalReadinessV1 {
                            ready: false,
                            code: Some("transfer_route_unavailable"),
                        });
                    }
                }
                _ => {}
            }
        }
        let managed_here = revision.steps.iter().any(|step| {
            matches!(
                step,
                PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. }
            ) && step_runs_on_host(revision, step, &self.local_host_ref)
        });
        if managed_here {
            let selection = match self.worker_provider_configs.selected_for_managed_workers() {
                Ok(selection) => selection,
                Err(_) => {
                    return Ok(LocalReadinessV1 {
                        ready: false,
                        code: Some("provider_unavailable"),
                    })
                }
            };
            let availability = self.managed_worker_plan_availability(revision, &selection)?;
            if revision.steps.iter().any(|step| {
                matches!(
                    step,
                    PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. }
                ) && step_runs_on_host(revision, step, &self.local_host_ref)
                    && !availability.supports(revision, step)
            }) {
                return Ok(LocalReadinessV1 {
                    ready: false,
                    code: Some("managed_platform_unavailable"),
                });
            }
        }
        Ok(LocalReadinessV1 {
            ready: true,
            code: None,
        })
    }
}

fn requester_has_local_step(
    revision: &PlanRevisionV2,
    participant: &PlanParticipantRef,
    local_host_ref: &HostRef,
) -> bool {
    &revision.requester == participant
        && revision
            .steps
            .iter()
            .any(|step| step_runs_on_host(revision, step, local_host_ref))
}

fn terminate_requester_attempt(
    runtime: &HostRuntime,
    attempt_id: &str,
    terminal_state: &str,
    code: &str,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    if !matches!(terminal_state, "failed" | "interrupted" | "cancelled") {
        return invalid("Native v2 terminal state is invalid.");
    }
    text(code, "terminal code")?;
    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    let row = tx
        .query_row(
            "SELECT a.approval_id, a.revision_id, a.revision_hash, a.expires_at,
                    r.revision_json
             FROM native_v2_product_attempts a
             JOIN native_v2_product_revisions r ON r.revision_id = a.revision_id
             WHERE a.attempt_id = ?1
             AND a.state IN ('checking_readiness','preparing','running')",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::InvalidInput("Native v2 attempt is already terminal.".into()))?;
    let revision: PlanRevisionV2 = serde_json::from_str(&row.4)?;
    tx.execute(
        "UPDATE native_v2_product_attempts SET state = ?2,
         failure_code = ?3, updated_at = ?4 WHERE attempt_id = ?1
         AND state IN ('checking_readiness','preparing','running')",
        params![attempt_id, terminal_state, code, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_revisions SET state = ?2,
         failure_code = ?3, updated_at = ?4 WHERE revision_id = ?1
         AND state IN ('checking_readiness','preparing','running')",
        params![row.1, terminal_state, code, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_steps SET state = ?2, updated_at = ?3
         WHERE attempt_id = ?1 AND state IN ('pending','eligible','running')",
        params![attempt_id, terminal_state, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_hosts SET admission_state = 'cancelled', updated_at = ?2
         WHERE attempt_id = ?1 AND admission_state IN ('pending','prepared','committed')",
        params![attempt_id, now],
    )?;
    let mut statement = tx.prepare(
        "SELECT participant_ref, peer_route_ref, session_binding_json
         FROM native_v2_product_hosts WHERE attempt_id = ?1 AND peer_route_ref IS NOT NULL
         ORDER BY participant_ref",
    )?;
    let rows = statement
        .query_map([attempt_id], |value| {
            Ok((
                value.get::<_, String>(0)?,
                value.get::<_, String>(1)?,
                value.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut events = Vec::new();
    for (participant_ref, peer_route_ref, binding_json) in rows {
        let binding: crate::host_identity::HostSessionBinding =
            serde_json::from_str(&binding_json)?;
        let current = crate::host_runtime::current_host_session_binding(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        binding.validate_current(&current, now)?;
        let target = revision
            .participants
            .as_slice()
            .iter()
            .find(|participant| participant.participant_ref.as_str() == participant_ref)
            .ok_or_else(|| AppError::InvalidInput("Native v2 cancel target vanished.".into()))?
            .participant_ref
            .clone();
        let cancel = NativeV2AttemptCancelV1 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-cancel-{}", uuid::Uuid::new_v4()),
            attempt_id: attempt_id.into(),
            approval_id: row.0.clone(),
            revision_id: row.1.clone(),
            revision_hash: row.2.clone(),
            bridge_id: revision.bridge_id.clone(),
            sender: revision.requester.clone(),
            target,
            reason_code: code.into(),
            expires_at: row.3,
        };
        let context = crate::room_control::room_control_session_context_for_peer(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        events.push(NativeV2OutboundEventV1 {
            room_id: revision.bridge_id.clone(),
            peer_route_ref,
            event: native_v2_control_event(CANCEL_KIND, serde_json::to_value(cancel)?, &context)?,
        });
    }
    tx.commit()?;
    Ok(events)
}

pub(crate) fn accept_receiver_cancel(
    runtime: &HostRuntime,
    cancel: &NativeV2AttemptCancelV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<Option<String>> {
    if cancel.protocol_version != PROTOCOL_VERSION
        || cancel.expires_at <= now
        || cancel.bridge_id != captured.bridge_id
        || !matches!(
            cancel.reason_code.as_str(),
            "user_cancelled" | "review_delivery_failed" | "coordination_delivery_failed"
        )
    {
        return invalid("Native v2 cancellation is invalid.");
    }
    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    let row = tx
        .query_row(
            "SELECT revision_id, revision_hash, approval_id, requester_participant_ref,
                    target_participant_ref, session_binding_json, state
             FROM native_v2_receiver_attempts WHERE attempt_id = ?1",
            [cancel.attempt_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 cancellation target is unavailable.".into())
        })?;
    let stored_binding: crate::host_identity::HostSessionBinding = serde_json::from_str(&row.5)?;
    if cancel.revision_id != row.0
        || cancel.revision_hash != row.1
        || cancel.approval_id != row.2
        || cancel.sender.as_str() != row.3
        || cancel.target.as_str() != row.4
        || !matches!(row.6.as_str(), "prepared" | "running")
        || &stored_binding != captured
    {
        return invalid("Native v2 cancellation crossed attempt/session authority.");
    }
    tx.execute(
        "UPDATE native_v2_receiver_attempts SET state = 'cancelled',
         failure_code = ?2, updated_at = ?3
         WHERE attempt_id = ?1 AND state IN ('prepared','running')",
        params![cancel.attempt_id, cancel.reason_code, now],
    )?;
    tx.execute(
        "UPDATE native_v2_external_dispatches SET state = 'cancelled',
         failure_code = ?2, updated_at = ?3
         WHERE attempt_id = ?1 AND state = 'dispatching'",
        params![cancel.attempt_id, cancel.reason_code, now],
    )?;
    let transfer_id = tx
        .query_row(
            "SELECT transfer_id FROM native_v2_external_dispatches
             WHERE attempt_id = ?1 AND state = 'cancelled' AND transfer_id IS NOT NULL LIMIT 1",
            [cancel.attempt_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    tx.commit()?;
    Ok(transfer_id)
}

/// Mirrors a terminal managed-Worker outcome into the coordinated receiver
/// attempt. This does not complete a product step or notify the requester; it
/// only prevents a locally failed/revoked receiver from remaining runnable if
/// the failure notification cannot cross the current session.
pub(crate) fn terminate_receiver_managed_attempt(
    paths: &AppPaths,
    attempt_id: &str,
    terminal_state: &str,
    code: &str,
    now: i64,
) -> AppResult<()> {
    if !matches!(terminal_state, "failed" | "interrupted" | "cancelled") {
        return invalid("Native v2 managed receiver terminal state is invalid.");
    }
    text(code, "managed receiver failure code")?;
    connection(paths)?.execute(
        "UPDATE native_v2_receiver_attempts SET state = ?2, failure_code = ?3,
         updated_at = ?4 WHERE attempt_id = ?1 AND state IN ('prepared','running')",
        params![attempt_id, terminal_state, code, now],
    )?;
    Ok(())
}

pub(crate) fn receiver_attempt_binding(
    paths: &AppPaths,
    attempt_id: &str,
) -> AppResult<Option<crate::host_identity::HostSessionBinding>> {
    connection(paths)?
        .query_row(
            "SELECT session_binding_json FROM native_v2_receiver_attempts
             WHERE attempt_id = ?1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| serde_json::from_str(&value).map_err(AppError::from))
        .transpose()
}

fn prepare_requester_attempt(
    runtime: &Arc<HostRuntime>,
    approval_id: &str,
    attempt_id: &str,
    expires_at: i64,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    id(attempt_id, "attempt id")?;
    if expires_at <= now || expires_at > now + MAX_LIFETIME_SECONDS {
        return invalid("Native v2 attempt expiry is invalid.");
    }
    let store = NativeV2ProductStore::new(&runtime.paths);
    let approval = store.approval(approval_id, now)?;
    let revision = store.revision(&approval.revision_id)?;
    if approval.expires_at < expires_at {
        return invalid("Native v2 attempt outlives requester approval.");
    }
    let mut prepared = Vec::new();
    for participant in revision.participants.as_slice() {
        if participant.host_ref == runtime.local_host_ref {
            let readiness =
                runtime.native_v2_local_readiness(&revision, &participant.participant_ref, now)?;
            if !readiness.ready {
                return invalid(readiness.code.unwrap_or("local_host_unavailable"));
            }
            prepared.push((participant.clone(), None, None, None));
            continue;
        }
        let binding =
            peer_binding_for_host(runtime, &revision.bridge_id, &participant.host_ref, now)?;
        let correlation_id = format!("native-v2-review-{}", uuid::Uuid::new_v4());
        let request_nonce = format!("native-v2-nonce-{}", uuid::Uuid::new_v4());
        let review = ReviewRequestV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-review-message-{}", uuid::Uuid::new_v4()),
            correlation_id: correlation_id.clone(),
            request_nonce: request_nonce.clone(),
            sender: revision.requester.clone(),
            target: participant.participant_ref.clone(),
            approval: approval.clone(),
            revision: revision.clone(),
        };
        let readiness = NativeV2ReadinessRequestV1 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-readiness-message-{}", uuid::Uuid::new_v4()),
            correlation_id,
            attempt_id: attempt_id.into(),
            approval_id: approval.approval_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            sender: revision.requester.clone(),
            target: participant.participant_ref.clone(),
            expires_at,
        };
        prepared.push((
            participant.clone(),
            Some(binding),
            Some(review),
            Some(readiness),
        ));
    }

    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO native_v2_product_attempts
         (attempt_id, approval_id, revision_id, revision_hash, state, failure_code,
          expires_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'checking_readiness', NULL, ?5, ?6, ?6)",
        params![
            attempt_id,
            approval.approval_id,
            revision.revision_id,
            revision.revision_hash,
            expires_at,
            now
        ],
    )?;
    tx.execute(
        "UPDATE native_v2_product_revisions SET state = 'checking_readiness',
         failure_code = NULL, updated_at = ?2 WHERE revision_id = ?1 AND state = 'approved'",
        params![revision.revision_id, now],
    )?;
    tx.execute(
        "DELETE FROM native_v2_product_steps WHERE revision_id = ?1 AND attempt_id IS NULL",
        [revision.revision_id.as_str()],
    )?;
    for step in &revision.steps {
        tx.execute(
            "INSERT INTO native_v2_product_steps
             (revision_id, attempt_id, step_id, operation, state, completion_ref, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', NULL, ?5)",
            params![
                revision.revision_id,
                attempt_id,
                step.id(),
                operation_name(step),
                now
            ],
        )?;
    }
    let mut outbound = Vec::new();
    for (participant, binding, review, readiness) in prepared {
        let is_local = binding.is_none();
        let readiness_state = if is_local { "ready" } else { "pending" };
        let admission_state = if is_local { "prepared" } else { "pending" };
        tx.execute(
            "INSERT INTO native_v2_product_hosts
             (attempt_id, participant_ref, host_ref, peer_route_ref, session_binding_ref,
              session_binding_json, review_correlation_id, review_request_nonce, review_json,
              start_json, readiness_state, admission_state, readiness_code,
              admission_ref, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, NULL, NULL, ?12)",
            params![
                attempt_id,
                participant.participant_ref.as_str(),
                participant.host_ref.as_str(),
                binding.as_ref().map(|value| value.peer_route_ref.as_str()),
                binding.as_ref().map(|value| value.binding_ref.as_str()),
                binding.as_ref().map(serde_json::to_string).transpose()?,
                review.as_ref().map(|value| value.correlation_id.as_str()),
                review.as_ref().map(|value| value.request_nonce.as_str()),
                review.as_ref().map(serde_json::to_string).transpose()?,
                readiness_state,
                admission_state,
                now
            ],
        )?;
        if let (Some(binding), Some(review), Some(readiness)) = (binding, review, readiness) {
            let context = crate::room_control::room_control_session_context_for_peer(
                runtime,
                &revision.bridge_id,
                &binding.peer_route_ref,
            )?;
            outbound.push(NativeV2OutboundEventV1 {
                room_id: revision.bridge_id.clone(),
                peer_route_ref: binding.peer_route_ref.clone(),
                event: native_v2_control_event(
                    "bridge_plan.v2.review_request",
                    serde_json::to_value(review)?,
                    &context,
                )?,
            });
            outbound.push(NativeV2OutboundEventV1 {
                room_id: revision.bridge_id.clone(),
                peer_route_ref: binding.peer_route_ref,
                event: native_v2_control_event(
                    READINESS_REQUEST_KIND,
                    serde_json::to_value(readiness)?,
                    &context,
                )?,
            });
        }
    }
    tx.commit()?;
    Ok(outbound)
}

async fn send_native_v2_event(
    runtime: Arc<HostRuntime>,
    outbound: NativeV2OutboundEventV1,
) -> AppResult<crate::room_control::RoomControlDeliveryReceipt> {
    let route =
        crate::room_control::selected_peer_route(&outbound.room_id, &outbound.peer_route_ref);
    crate::room_control::send_room_control_event(
        runtime,
        &outbound.room_id,
        outbound.event,
        Some(route),
    )
    .await
}

fn peer_binding_for_host(
    runtime: &HostRuntime,
    bridge_id: &str,
    host_ref: &HostRef,
    now: i64,
) -> AppResult<crate::host_identity::HostSessionBinding> {
    let peers = crate::storage::list_bridge_peer_endpoints(&runtime.paths, bridge_id)?;
    let mut matches = peers.into_iter().filter(|peer| {
        peer.logical_host_ref.as_deref() == Some(host_ref.as_str())
            && peer.liveness == crate::models::BridgePeerLiveness::Connected
    });
    let peer = matches
        .next()
        .ok_or_else(|| AppError::InvalidInput("Plan Host route is unavailable.".into()))?;
    if matches.next().is_some() {
        return invalid("Plan Host route is ambiguous.");
    }
    let binding = crate::host_runtime::current_host_session_binding(
        runtime,
        bridge_id,
        &peer.peer_session_id,
    )?;
    if binding.expires_at <= now || binding.peer_host_ref != *host_ref {
        return invalid("Plan Host session binding is unavailable.");
    }
    Ok(binding)
}

fn native_v2_control_event(
    kind: &str,
    payload: Value,
    context: &crate::room_control::RoomControlSessionContext,
) -> AppResult<Value> {
    use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
    let now = OffsetDateTime::now_utc();
    Ok(serde_json::json!({
        "schemaVersion": "pastey-room-control-event-v1",
        "eventId": format!("native-v2-event-{}", uuid::Uuid::new_v4()),
        "kind": kind,
        "protocolFamily": "bridge_plan",
        "roomRef": context.room_id,
        "sourceDeviceRef": context.local_session_ref,
        "targetPeerRef": context.peer_session_ref,
        "createdAt": now.format(&Rfc3339).map_err(|_| AppError::InvalidInput("Unable to format native v2 event time.".into()))?,
        "expiresAt": (now + Duration::seconds(120)).format(&Rfc3339).map_err(|_| AppError::InvalidInput("Unable to format native v2 event time.".into()))?,
        "previewOnly": false,
        "payload": payload,
    }))
}

pub(crate) fn control_event_for_session(
    kind: &str,
    payload: Value,
    context: &crate::room_control::RoomControlSessionContext,
) -> AppResult<Value> {
    native_v2_control_event(kind, payload, context)
}

pub(crate) fn start_receiver_attempt(
    runtime: Arc<HostRuntime>,
    attempt_id: String,
    captured: crate::host_identity::HostSessionBinding,
) {
    let task_runtime = runtime.clone();
    runtime.spawn(async move {
        drive_receiver_attempt(task_runtime, attempt_id, captured).await;
    });
}

async fn drive_receiver_attempt(
    runtime: Arc<HostRuntime>,
    attempt_id: String,
    captured: crate::host_identity::HostSessionBinding,
) {
    let Ok((revision, _)) = load_receiver_attempt(&runtime.paths, &attempt_id) else {
        return;
    };
    let Ok(conn) = connection(&runtime.paths) else {
        return;
    };
    for step in &revision.steps {
        if committed_step(&conn, &attempt_id, step.id()).unwrap_or(false) {
            continue;
        }
        if step
            .dependencies()
            .iter()
            .any(|dependency| !committed_step(&conn, &attempt_id, dependency).unwrap_or(false))
        {
            continue;
        }
        if !step_runs_on_host(&revision, step, &runtime.local_host_ref) {
            return;
        }
        match step {
            PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. } => {
                runtime.drive_live_v2_attempt(attempt_id, captured);
            }
            PlanStepV2::Search { .. } | PlanStepV2::Transfer { .. } => {
                // External authored primitives attach below. Keeping the
                // dispatch here centralized prevents the Worker from owning
                // continuation even while those adapters are running.
                let _ = execute_external_step(runtime, attempt_id, captured, step.clone()).await;
            }
        }
        return;
    }
}

async fn execute_external_step(
    runtime: Arc<HostRuntime>,
    attempt_id: String,
    captured: crate::host_identity::HostSessionBinding,
    step: PlanStepV2,
) -> AppResult<()> {
    let now = crate::storage::now_ts();
    reserve_external_dispatch(&runtime.paths, &attempt_id, &step, now)?;
    let result = match &step {
        PlanStepV2::Search {
            host,
            output,
            query,
            safe_scope_labels,
            ..
        } => execute_authored_search(
            &runtime,
            &attempt_id,
            &captured,
            &step,
            host,
            output,
            query,
            safe_scope_labels,
            now,
        ),
        PlanStepV2::Transfer {
            source,
            destination,
            input,
            ..
        } => {
            execute_authored_transfer(
                runtime.clone(),
                &attempt_id,
                &captured,
                &step,
                source,
                destination,
                input,
                now,
            )
            .await
        }
        _ => invalid("Native v2 external dispatcher received a managed step."),
    };
    match result {
        Ok(result) => {
            complete_external_dispatch(
                &runtime.paths,
                &attempt_id,
                step.id(),
                crate::storage::now_ts(),
            )?;
            submit_remote_step_result(runtime, captured, result).await
        }
        Err(error) => {
            fail_receiver_external(
                &runtime.paths,
                &attempt_id,
                step.id(),
                "external_step_failed",
                crate::storage::now_ts(),
            )?;
            let _ = submit_remote_attempt_failure(
                runtime,
                captured,
                &attempt_id,
                Some(step.id()),
                "external_step_failed",
            )
            .await;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_authored_search(
    runtime: &HostRuntime,
    attempt_id: &str,
    captured: &crate::host_identity::HostSessionBinding,
    step: &PlanStepV2,
    host: &PlanParticipantRef,
    output: &ManagedObjectRevisionV2,
    query: &str,
    safe_scope_labels: &[String],
    now: i64,
) -> AppResult<NativeV2StepResultV1> {
    use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
    let (revision, approval) = load_receiver_attempt(&runtime.paths, attempt_id)?;
    if !step_runs_on_host(&revision, step, &runtime.local_host_ref) {
        return invalid("Native v2 Search was dispatched at the wrong Host.");
    }
    let request = crate::file_candidates::BridgePlanSearchRequest {
        request_id: format!("bridge-plan-request-{attempt_id}"),
        room_ref: revision.bridge_id.clone(),
        requester_device_ref: revision.requester.as_str().into(),
        receiver_device_ref: host.as_str().into(),
        filename_hint: query.into(),
        extensions: Vec::new(),
        safe_scope_labels: safe_scope_labels.to_vec(),
        expires_at: (OffsetDateTime::now_utc() + Duration::seconds(120))
            .format(&Rfc3339)
            .map_err(|_| AppError::InvalidInput("Native v2 Search time is invalid.".into()))?,
    };
    let search = {
        let mut candidates = runtime.bridge_plan_candidate_store.lock();
        crate::file_candidates::execute_bridge_plan_search_and_store(
            request,
            &runtime.paths,
            &mut candidates,
        )?
    };
    if search.status != "completed" || search.candidates.len() != 1 {
        return invalid("Native v2 Search must resolve exactly one bounded candidate.");
    }
    let candidate = &search.candidates[0];
    let private_file = {
        let mut candidates = runtime.bridge_plan_candidate_store.lock();
        crate::file_candidates::resolve_bridge_plan_selected_file(
            &mut candidates,
            &revision.bridge_id,
            revision.requester.as_str(),
            host.as_str(),
            attempt_id,
            &candidate.candidate_id,
        )?
    };
    let acquisition = runtime
        .managed_objects
        .lock()
        .bind_authored_search_revision(
            crate::managed_objects::HostArtifactAcquisition {
                kind: crate::managed_objects::ManagedObjectAcquisitionKind::SearchResult,
                source_ref: format!("native-v2-search:{attempt_id}:{}", step.id()),
                bridge_id: Some(revision.bridge_id.clone()),
                path: private_file.path,
                scope_root: private_file.scope_root,
                display_name: private_file.display_name,
                media_type: private_file.mime_type,
                expires_at: approval.expires_at,
                app_owned_temporary: private_file.app_owned_temporary,
            },
            output.logical_object_id.clone(),
            output.revision,
            now,
        )?;
    let artifact = runtime.managed_objects.lock().resolve(&acquisition, now)?;
    build_step_result(
        &revision,
        &approval,
        attempt_id,
        step,
        host.clone(),
        runtime.local_host_ref.clone(),
        Some(output.clone()),
        Some(artifact.identity.digest),
        None,
        captured.binding_ref.clone(),
        now,
    )
}

#[allow(clippy::too_many_arguments)]
async fn execute_authored_transfer(
    runtime: Arc<HostRuntime>,
    attempt_id: &str,
    captured: &crate::host_identity::HostSessionBinding,
    step: &PlanStepV2,
    source: &PlanParticipantRef,
    destination: &PlanParticipantRef,
    input: &ManagedObjectRevisionV2,
    now: i64,
) -> AppResult<NativeV2StepResultV1> {
    let (revision, approval) = load_receiver_attempt(&runtime.paths, attempt_id)?;
    if !step_runs_on_host(&revision, step, &runtime.local_host_ref) {
        return invalid("Native v2 Transfer was dispatched at the wrong Host.");
    }
    let destination_host = participant_for_ref(&revision, destination)
        .ok_or_else(|| AppError::InvalidInput("Native v2 Transfer destination vanished.".into()))?
        .host_ref
        .clone();
    let binding = peer_binding_for_host(&runtime, &revision.bridge_id, &destination_host, now)?;
    let peer = crate::storage::list_bridge_peer_endpoints(&runtime.paths, &revision.bridge_id)?
        .into_iter()
        .find(|peer| peer.peer_session_id == binding.peer_route_ref)
        .ok_or_else(|| AppError::InvalidInput("Native v2 Transfer route vanished.".into()))?;
    let endpoint = crate::transfer::BridgePeerTransferEndpoint {
        peer_session_id: peer.peer_session_id,
        host: peer.endpoint_host.ok_or_else(|| {
            AppError::InvalidInput("Native v2 Transfer endpoint is unavailable.".into())
        })?,
        port: peer.endpoint_port.ok_or_else(|| {
            AppError::InvalidInput("Native v2 Transfer endpoint is unavailable.".into())
        })?,
        transport_public_key: peer.transport_public_key.ok_or_else(|| {
            AppError::InvalidInput("Native v2 Transfer transport identity is unavailable.".into())
        })?,
    };
    let acquisition = runtime.managed_objects.lock().acquisition_for_revision(
        &revision.bridge_id,
        &input.logical_object_id,
        input.revision,
        now,
    )?;
    let artifact = runtime.managed_objects.lock().resolve(&acquisition, now)?;
    let master_key = {
        let config = runtime.config.read();
        crate::config::master_key(&config)?
    };
    let item = crate::storage::create_outgoing_file_item_with_metadata(
        &runtime.paths,
        &master_key,
        &revision.bridge_id,
        &artifact.path,
        Some(artifact.display_name.clone()),
        Some(artifact.media_type.clone()),
    )?;
    set_external_transfer_id(&runtime.paths, attempt_id, step.id(), &item.id, now)?;
    let source_host = participant_for_ref(&revision, source)
        .ok_or_else(|| AppError::InvalidInput("Native v2 Transfer source vanished.".into()))?
        .host_ref
        .clone();
    let metadata = NativeV2TransferMetadataV1 {
        protocol_version: PROTOCOL_VERSION.into(),
        attempt_id: attempt_id.into(),
        approval_id: approval.approval_id.clone(),
        plan_id: revision.plan_id.clone(),
        revision_id: revision.revision_id.clone(),
        revision_hash: revision.revision_hash.clone(),
        bridge_id: revision.bridge_id.clone(),
        step_id: step.id().into(),
        source: source.clone(),
        destination: destination.clone(),
        source_host_ref: source_host,
        destination_host_ref: destination_host,
        object: input.clone(),
        content_digest: artifact.identity.digest.clone(),
        expires_at: approval.expires_at,
    };
    let transfer_result = crate::transfer::send_native_v2_managed_revision_to_bridge_peer_endpoint(
        runtime.clone(),
        &revision.bridge_id,
        &item.id,
        &artifact.path,
        Some(format!("native-v2:{attempt_id}:{}", step.id())),
        None,
        endpoint,
        metadata,
    )
    .await;
    let _ = crate::storage::delete_room_item(&runtime.paths, &item.id);
    transfer_result?;
    build_step_result(
        &revision,
        &approval,
        attempt_id,
        step,
        source.clone(),
        runtime.local_host_ref.clone(),
        Some(input.clone()),
        Some(artifact.identity.digest),
        None,
        captured.binding_ref.clone(),
        crate::storage::now_ts(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_step_result(
    revision: &PlanRevisionV2,
    approval: &PlanApprovalV2,
    attempt_id: &str,
    step: &PlanStepV2,
    participant: PlanParticipantRef,
    host_ref: HostRef,
    object: Option<ManagedObjectRevisionV2>,
    content_digest: Option<String>,
    result_digest: Option<String>,
    session_binding_ref: String,
    completed_at: i64,
) -> AppResult<NativeV2StepResultV1> {
    let mut result = NativeV2StepResultV1 {
        protocol_version: PROTOCOL_VERSION.into(),
        message_id: format!("native-v2-step-result-{}", uuid::Uuid::new_v4()),
        attempt_id: attempt_id.into(),
        approval_id: approval.approval_id.clone(),
        plan_id: revision.plan_id.clone(),
        revision_id: revision.revision_id.clone(),
        revision_hash: revision.revision_hash.clone(),
        bridge_id: revision.bridge_id.clone(),
        step_id: step.id().into(),
        operation: step.operation(),
        participant,
        host_ref,
        object,
        content_digest,
        result_digest,
        session_binding_ref,
        completion_ref: String::new(),
        completed_at,
    };
    result.completion_ref = step_result_completion_ref(&result)?;
    validate_step_result(revision, approval, attempt_id, &result)?;
    Ok(result)
}

pub(crate) async fn submit_remote_step_result(
    runtime: Arc<HostRuntime>,
    captured: crate::host_identity::HostSessionBinding,
    result: NativeV2StepResultV1,
) -> AppResult<()> {
    let current = crate::host_runtime::current_host_session_binding(
        &runtime,
        &captured.bridge_id,
        &captured.peer_route_ref,
    )?;
    captured.validate_current(&current, crate::storage::now_ts())?;
    let context = crate::room_control::room_control_session_context_for_peer(
        &runtime,
        &captured.bridge_id,
        &captured.peer_route_ref,
    )?;
    let event = native_v2_control_event(STEP_RESULT_KIND, serde_json::to_value(result)?, &context)?;
    send_native_v2_event(
        runtime,
        NativeV2OutboundEventV1 {
            room_id: captured.bridge_id,
            peer_route_ref: captured.peer_route_ref,
            event,
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn submit_remote_attempt_failure(
    runtime: Arc<HostRuntime>,
    captured: crate::host_identity::HostSessionBinding,
    attempt_id: &str,
    step_id: Option<&str>,
    code: &str,
) -> AppResult<()> {
    text(code, "failure code")?;
    let current = crate::host_runtime::current_host_session_binding(
        &runtime,
        &captured.bridge_id,
        &captured.peer_route_ref,
    )?;
    captured.validate_current(&current, crate::storage::now_ts())?;
    let (revision, approval) = load_receiver_attempt(&runtime.paths, attempt_id)?;
    let participant = revision
        .participants
        .as_slice()
        .iter()
        .find(|participant| participant.host_ref == runtime.local_host_ref)
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 failure participant is unavailable.".into())
        })?;
    if let Some(step_id) = step_id {
        let step = revision
            .steps
            .iter()
            .find(|step| step.id() == step_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native v2 failure step is unavailable.".into())
            })?;
        if !step_runs_on_host(&revision, step, &runtime.local_host_ref) {
            return invalid("Native v2 failure step belongs to another Host.");
        }
    }
    let failure = NativeV2StepFailureV1 {
        protocol_version: PROTOCOL_VERSION.into(),
        message_id: format!("native-v2-step-failure-{}", uuid::Uuid::new_v4()),
        attempt_id: attempt_id.into(),
        approval_id: approval.approval_id,
        revision_id: revision.revision_id,
        revision_hash: revision.revision_hash,
        bridge_id: revision.bridge_id.clone(),
        step_id: step_id.map(str::to_string),
        participant: participant.participant_ref.clone(),
        host_ref: runtime.local_host_ref.clone(),
        session_binding_ref: captured.binding_ref.clone(),
        code: code.into(),
        expires_at: approval.expires_at,
    };
    let context = crate::room_control::room_control_session_context_for_peer(
        &runtime,
        &revision.bridge_id,
        &captured.peer_route_ref,
    )?;
    let event =
        native_v2_control_event(STEP_FAILURE_KIND, serde_json::to_value(failure)?, &context)?;
    send_native_v2_event(
        runtime,
        NativeV2OutboundEventV1 {
            room_id: revision.bridge_id,
            peer_route_ref: captured.peer_route_ref,
            event,
        },
    )
    .await?;
    Ok(())
}

pub(crate) fn accept_requester_step_failure(
    runtime: &Arc<HostRuntime>,
    failure: NativeV2StepFailureV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    if failure.protocol_version != PROTOCOL_VERSION
        || failure.expires_at <= now
        || failure.bridge_id != captured.bridge_id
        || failure.host_ref != captured.peer_host_ref
        || failure.session_binding_ref != captured.binding_ref
        || failure.code.trim().is_empty()
        || failure.code.len() > 128
    {
        return invalid("Native v2 failure Host/session is invalid.");
    }
    let row = connection(&runtime.paths)?
        .query_row(
            "SELECT a.revision_id, a.revision_hash, a.approval_id, h.session_binding_json
             FROM native_v2_product_attempts a
             JOIN native_v2_product_hosts h ON h.attempt_id = a.attempt_id
                  AND h.participant_ref = ?2
             WHERE a.attempt_id = ?1 AND a.state IN
                  ('checking_readiness','preparing','running')",
            params![failure.attempt_id, failure.participant.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::InvalidInput("Native v2 failure is late or unknown.".into()))?;
    let stored_binding: crate::host_identity::HostSessionBinding =
        serde_json::from_str(row.3.as_deref().ok_or_else(|| {
            AppError::InvalidInput("Native v2 failure Host is unavailable.".into())
        })?)?;
    if row.0 != failure.revision_id
        || row.1 != failure.revision_hash
        || row.2 != failure.approval_id
        || &stored_binding != captured
    {
        return invalid("Native v2 failure crossed immutable attempt correlation.");
    }
    if let Some(step_id) = failure.step_id.as_deref() {
        let revision = NativeV2ProductStore::new(&runtime.paths).revision(&row.0)?;
        let step = revision
            .steps
            .iter()
            .find(|step| step.id() == step_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native v2 failure step is unavailable.".into())
            })?;
        if !step_completed_by(step, &failure.participant) {
            return invalid("Native v2 failure step belongs to another Host.");
        }
    }
    terminate_requester_attempt(runtime, &failure.attempt_id, "failed", &failure.code, now)
}

pub(crate) fn managed_step_result(
    runtime: &HostRuntime,
    attempt_id: &str,
    step: &PlanStepV2,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<NativeV2StepResultV1> {
    let (revision, approval) = load_receiver_attempt(&runtime.paths, attempt_id)?;
    if !step_runs_on_host(&revision, step, &runtime.local_host_ref) {
        return invalid("Managed native v2 result was produced at the wrong Host.");
    }
    let participant = revision
        .participants
        .as_slice()
        .iter()
        .find(|participant| participant.host_ref == runtime.local_host_ref)
        .ok_or_else(|| AppError::InvalidInput("Managed result participant is unavailable.".into()))?
        .participant_ref
        .clone();
    let conn = connection(&runtime.paths)?;
    match step {
        PlanStepV2::Transform { output, .. } => {
            let row = conn
                .query_row(
                    "SELECT r.logical_object_id, r.output_revision, r.content_digest
                     FROM bridge_plan_v2_transform_results r
                     JOIN bridge_plan_v2_managed_step_claims c
                       ON c.attempt_id = r.attempt_id AND c.step_id = r.step_id
                     WHERE r.attempt_id = ?1 AND r.step_id = ?2 AND c.state = 'completed'",
                    params![attempt_id, step.id()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    AppError::InvalidInput("Core Transform result is unavailable.".into())
                })?;
            if row.0 != output.logical_object_id || row.1 != output.revision {
                return invalid("Core Transform result does not match the authored output.");
            }
            build_step_result(
                &revision,
                &approval,
                attempt_id,
                step,
                participant,
                runtime.local_host_ref.clone(),
                Some(output.clone()),
                Some(row.2),
                None,
                captured.binding_ref.clone(),
                now,
            )
        }
        PlanStepV2::Execute { .. } => {
            let digest = conn
                .query_row(
                    "SELECT r.result_digest FROM bridge_plan_v2_execute_results r
                     JOIN bridge_plan_v2_managed_step_claims c
                       ON c.attempt_id = r.attempt_id AND c.step_id = r.step_id
                     WHERE r.attempt_id = ?1 AND r.step_id = ?2 AND c.state = 'completed'",
                    params![attempt_id, step.id()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    AppError::InvalidInput("Core Execute result is unavailable.".into())
                })?;
            build_step_result(
                &revision,
                &approval,
                attempt_id,
                step,
                participant,
                runtime.local_host_ref.clone(),
                None,
                None,
                Some(digest),
                captured.binding_ref.clone(),
                now,
            )
        }
        _ => invalid("Only managed steps have Core managed results."),
    }
}

fn reserve_external_dispatch(
    paths: &AppPaths,
    attempt_id: &str,
    step: &PlanStepV2,
    now: i64,
) -> AppResult<()> {
    let conn = connection(paths)?;
    let active: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_receiver_attempts
         WHERE attempt_id = ?1 AND state = 'running')",
        [attempt_id],
        |row| row.get(0),
    )?;
    if active != 1 {
        return invalid("Native v2 receiver attempt is not running.");
    }
    conn.execute(
        "INSERT INTO native_v2_external_dispatches
         (attempt_id, step_id, operation, state, transfer_id, failure_code,
          created_at, updated_at)
         VALUES (?1, ?2, ?3, 'dispatching', NULL, NULL, ?4, ?4)",
        params![attempt_id, step.id(), operation_name(step), now],
    )?;
    Ok(())
}

fn set_external_transfer_id(
    paths: &AppPaths,
    attempt_id: &str,
    step_id: &str,
    transfer_id: &str,
    now: i64,
) -> AppResult<()> {
    let changed = connection(paths)?.execute(
        "UPDATE native_v2_external_dispatches SET transfer_id = ?3, updated_at = ?4
         WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'dispatching'
         AND transfer_id IS NULL",
        params![attempt_id, step_id, transfer_id, now],
    )?;
    if changed != 1 {
        return invalid("Native v2 Transfer dispatch is unavailable.");
    }
    Ok(())
}

fn complete_external_dispatch(
    paths: &AppPaths,
    attempt_id: &str,
    step_id: &str,
    now: i64,
) -> AppResult<()> {
    let changed = connection(paths)?.execute(
        "UPDATE native_v2_external_dispatches SET state = 'completed', updated_at = ?3
         WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'dispatching'",
        params![attempt_id, step_id, now],
    )?;
    if changed != 1 {
        return invalid("Native v2 external completion is late or replayed.");
    }
    Ok(())
}

fn fail_receiver_external(
    paths: &AppPaths,
    attempt_id: &str,
    step_id: &str,
    code: &str,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE native_v2_external_dispatches SET state = 'failed', failure_code = ?3,
         updated_at = ?4 WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'dispatching'",
        params![attempt_id, step_id, code, now],
    )?;
    tx.execute(
        "UPDATE native_v2_receiver_attempts SET state = 'failed', failure_code = ?2,
         updated_at = ?3 WHERE attempt_id = ?1 AND state = 'running'",
        params![attempt_id, code, now],
    )?;
    tx.commit()?;
    Ok(())
}

fn fail_requester_attempt(
    paths: &AppPaths,
    attempt_id: &str,
    code: &str,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let revision_id: String = tx.query_row(
        "SELECT revision_id FROM native_v2_product_attempts WHERE attempt_id = ?1",
        [attempt_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE native_v2_product_attempts SET state = 'failed', failure_code = ?2,
         updated_at = ?3 WHERE attempt_id = ?1
         AND state IN ('checking_readiness','preparing','running')",
        params![attempt_id, code, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_revisions SET state = 'failed', failure_code = ?2,
         updated_at = ?3 WHERE revision_id = ?1
         AND state IN ('checking_readiness','preparing','running')",
        params![revision_id, code, now],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn accept_readiness_request(
    runtime: &HostRuntime,
    request: NativeV2ReadinessRequestV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<NativeV2ReadinessV1> {
    if request.protocol_version != PROTOCOL_VERSION
        || request.expires_at <= now
        || request.bridge_id != captured.bridge_id
        || request.approval_id.trim().is_empty()
    {
        return invalid("Native v2 readiness request is invalid.");
    }
    let review: ReviewRequestV2 = connection(&runtime.paths)?
        .query_row(
            "SELECT review_json FROM bridge_plan_v2_protocol_reviews
             WHERE bridge_id = ?1 AND correlation_id = ?2",
            params![request.bridge_id, request.correlation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|json| serde_json::from_str(&json))
        .transpose()?
        .ok_or_else(|| AppError::InvalidInput("Native v2 reviewed Plan is unavailable.".into()))?;
    if request.approval_id != review.approval.approval_id
        || request.plan_id != review.revision.plan_id
        || request.revision_id != review.revision.revision_id
        || request.revision_hash != review.revision.revision_hash
        || request.bridge_id != review.revision.bridge_id
        || request.sender != review.sender
        || request.target != review.target
        || request.expires_at > review.approval.expires_at
        || captured.peer_host_ref
            != participant_for_ref(&review.revision, &review.sender)
                .ok_or_else(|| {
                    AppError::InvalidInput("Native v2 requester is unavailable.".into())
                })?
                .host_ref
        || captured.local_host_ref
            != participant_for_ref(&review.revision, &review.target)
                .ok_or_else(|| AppError::InvalidInput("Native v2 target is unavailable.".into()))?
                .host_ref
    {
        return invalid("Native v2 readiness crossed reviewed Plan or Host session correlation.");
    }
    let readiness = runtime.native_v2_local_readiness(&review.revision, &review.target, now)?;
    connection(&runtime.paths)?.execute(
        "INSERT INTO native_v2_receiver_reviews
         (correlation_id, attempt_id, revision_id, revision_hash, approval_id,
          requester_participant_ref, target_participant_ref, session_binding_json,
          readiness_state, readiness_code, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            request.correlation_id,
            request.attempt_id,
            request.revision_id,
            request.revision_hash,
            request.approval_id,
            request.sender.as_str(),
            request.target.as_str(),
            serde_json::to_string(captured)?,
            if readiness.ready {
                "ready"
            } else {
                "unavailable"
            },
            readiness.code,
            request.expires_at,
            now
        ],
    )?;
    Ok(NativeV2ReadinessV1 {
        protocol_version: PROTOCOL_VERSION.into(),
        message_id: format!("native-v2-readiness-result-{}", uuid::Uuid::new_v4()),
        correlation_id: request.correlation_id,
        attempt_id: request.attempt_id,
        approval_id: request.approval_id,
        plan_id: request.plan_id,
        revision_id: request.revision_id,
        revision_hash: request.revision_hash,
        bridge_id: request.bridge_id,
        participant: request.target,
        host_ref: runtime.local_host_ref.clone(),
        session_binding_ref: captured.binding_ref.clone(),
        ready: readiness.ready,
        code: readiness.code.map(str::to_string),
        expires_at: request.expires_at,
    })
}

pub(crate) fn accept_requester_readiness(
    runtime: &Arc<HostRuntime>,
    result: NativeV2ReadinessV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    if result.protocol_version != PROTOCOL_VERSION
        || result.expires_at <= now
        || result.bridge_id != captured.bridge_id
        || result.host_ref != captured.peer_host_ref
        || runtime.local_host_ref != captured.local_host_ref
    {
        return invalid("Native v2 readiness result Host/session is invalid.");
    }
    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    let row = tx
        .query_row(
            "SELECT a.revision_id, a.revision_hash, a.approval_id, a.expires_at,
                    h.session_binding_json, h.review_correlation_id, h.readiness_state
             FROM native_v2_product_attempts a
             JOIN native_v2_product_hosts h ON h.attempt_id = a.attempt_id
             WHERE a.attempt_id = ?1 AND h.participant_ref = ?2
             AND a.state = 'checking_readiness'",
            params![result.attempt_id, result.participant.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 readiness result is late or unknown.".into())
        })?;
    let stored_binding: crate::host_identity::HostSessionBinding =
        serde_json::from_str(row.4.as_deref().ok_or_else(|| {
            AppError::InvalidInput("Native v2 Host binding is unavailable.".into())
        })?)?;
    if row.0 != result.revision_id
        || row.1 != result.revision_hash
        || row.2 != result.approval_id
        || row.3 < result.expires_at
        || row.5.as_deref() != Some(result.correlation_id.as_str())
        || result.session_binding_ref != stored_binding.binding_ref
        || &stored_binding != captured
        || row.6 != "pending"
    {
        return invalid("Native v2 readiness result correlation is invalid or replayed.");
    }
    tx.execute(
        "UPDATE native_v2_product_hosts SET readiness_state = ?3,
         readiness_code = ?4, updated_at = ?5
         WHERE attempt_id = ?1 AND participant_ref = ?2 AND readiness_state = 'pending'",
        params![
            result.attempt_id,
            result.participant.as_str(),
            if result.ready { "ready" } else { "unavailable" },
            result.code,
            now
        ],
    )?;
    if !result.ready {
        let revision_id = row.0;
        tx.execute(
            "UPDATE native_v2_product_attempts SET state = 'failed',
             failure_code = 'whole_plan_unavailable', updated_at = ?2
             WHERE attempt_id = ?1 AND state = 'checking_readiness'",
            params![result.attempt_id, now],
        )?;
        tx.execute(
            "UPDATE native_v2_product_revisions SET state = 'failed',
             failure_code = 'whole_plan_unavailable', updated_at = ?2
             WHERE revision_id = ?1 AND state = 'checking_readiness'",
            params![revision_id, now],
        )?;
        tx.commit()?;
        return Ok(Vec::new());
    }
    let pending: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_v2_product_hosts
         WHERE attempt_id = ?1 AND readiness_state != 'ready'",
        [result.attempt_id.as_str()],
        |value| value.get(0),
    )?;
    if pending != 0 {
        tx.commit()?;
        return Ok(Vec::new());
    }
    let revision_json: String = tx.query_row(
        "SELECT r.revision_json FROM native_v2_product_revisions r
         JOIN native_v2_product_attempts a ON a.revision_id = r.revision_id
         WHERE a.attempt_id = ?1",
        [result.attempt_id.as_str()],
        |value| value.get(0),
    )?;
    let approval_json: String = tx.query_row(
        "SELECT p.approval_json FROM native_v2_product_approvals p
         JOIN native_v2_product_attempts a ON a.approval_id = p.approval_id
         WHERE a.attempt_id = ?1",
        [result.attempt_id.as_str()],
        |value| value.get(0),
    )?;
    let revision: PlanRevisionV2 = serde_json::from_str(&revision_json)?;
    let approval: PlanApprovalV2 = serde_json::from_str(&approval_json)?;
    let mut statement = tx.prepare(
        "SELECT participant_ref, peer_route_ref, session_binding_json, review_json
         FROM native_v2_product_hosts WHERE attempt_id = ?1 AND peer_route_ref IS NOT NULL
         ORDER BY participant_ref",
    )?;
    let rows = statement
        .query_map([result.attempt_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut events = Vec::new();
    for (participant_ref, peer_route_ref, binding_json, review_json) in rows {
        let binding: crate::host_identity::HostSessionBinding =
            serde_json::from_str(&binding_json)?;
        let current = crate::host_runtime::current_host_session_binding(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        binding.validate_current(&current, now)?;
        let review: ReviewRequestV2 = serde_json::from_str(&review_json)?;
        let start = AttemptStartV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-start-message-{}", uuid::Uuid::new_v4()),
            correlation_id: review.correlation_id.clone(),
            request_nonce: review.request_nonce.clone(),
            attempt_id: result.attempt_id.clone(),
            approval_id: approval.approval_id.clone(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            sender: revision.requester.clone(),
            target: revision
                .participants
                .as_slice()
                .iter()
                .find(|participant| participant.participant_ref.as_str() == participant_ref)
                .ok_or_else(|| AppError::InvalidInput("Native v2 target vanished.".into()))?
                .participant_ref
                .clone(),
            expires_at: row.3,
        };
        tx.execute(
            "UPDATE native_v2_product_hosts SET start_json = ?3, updated_at = ?4
             WHERE attempt_id = ?1 AND participant_ref = ?2",
            params![
                result.attempt_id,
                participant_ref,
                serde_json::to_string(&start)?,
                now
            ],
        )?;
        let context = crate::room_control::room_control_session_context_for_peer(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        events.push(NativeV2OutboundEventV1 {
            room_id: revision.bridge_id.clone(),
            peer_route_ref,
            event: native_v2_control_event(
                "bridge_plan.v2.attempt_start",
                serde_json::to_value(start)?,
                &context,
            )?,
        });
    }
    tx.execute(
        "UPDATE native_v2_product_attempts SET state = 'preparing', updated_at = ?2
         WHERE attempt_id = ?1 AND state = 'checking_readiness'",
        params![result.attempt_id, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_revisions SET state = 'preparing', updated_at = ?2
         WHERE revision_id = ?1 AND state = 'checking_readiness'",
        params![revision.revision_id, now],
    )?;
    tx.commit()?;
    Ok(events)
}

pub(crate) fn coordinated_receiver_review(
    paths: &AppPaths,
    correlation_id: &str,
    attempt_id: &str,
) -> AppResult<bool> {
    Ok(connection(paths)?.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_receiver_reviews
         WHERE correlation_id = ?1 AND attempt_id = ?2 AND readiness_state = 'ready')",
        params![correlation_id, attempt_id],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn record_receiver_prepared(
    paths: &AppPaths,
    start: &AttemptStartV2,
    accepted: &crate::bridge_plan_v2::AcceptedAttemptV2,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<NativeV2PreparedV1> {
    let review = connection(paths)?
        .query_row(
            "SELECT requester_participant_ref, target_participant_ref,
                    session_binding_json, expires_at
             FROM native_v2_receiver_reviews
             WHERE correlation_id = ?1 AND attempt_id = ?2 AND readiness_state = 'ready'",
            params![start.correlation_id, start.attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 prepared review is unavailable.".into())
        })?;
    let reviewed_binding: crate::host_identity::HostSessionBinding =
        serde_json::from_str(&review.2)?;
    if &reviewed_binding != captured
        || review.0 != start.sender.as_str()
        || review.1 != start.target.as_str()
        || review.3 < start.expires_at
    {
        return invalid("Native v2 prepared attempt changed Host session or review binding.");
    }
    connection(paths)?.execute(
        "INSERT INTO native_v2_receiver_attempts
         (attempt_id, revision_id, revision_hash, approval_id,
          requester_participant_ref, target_participant_ref, session_binding_ref,
          session_binding_json,
          state, failure_code, expires_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'prepared', NULL, ?9, ?10, ?10)",
        params![
            start.attempt_id,
            start.revision_id,
            start.revision_hash,
            start.approval_id,
            start.sender.as_str(),
            start.target.as_str(),
            captured.binding_ref,
            serde_json::to_string(captured)?,
            start.expires_at,
            now
        ],
    )?;
    Ok(NativeV2PreparedV1 {
        protocol_version: PROTOCOL_VERSION.into(),
        message_id: format!("native-v2-prepared-{}", uuid::Uuid::new_v4()),
        correlation_id: start.correlation_id.clone(),
        attempt_id: start.attempt_id.clone(),
        approval_id: start.approval_id.clone(),
        plan_id: start.plan_id.clone(),
        revision_id: start.revision_id.clone(),
        revision_hash: start.revision_hash.clone(),
        bridge_id: start.bridge_id.clone(),
        participant: start.target.clone(),
        host_ref: captured.local_host_ref.clone(),
        admission_ref: accepted.admission_ref.clone(),
        session_binding_ref: captured.binding_ref.clone(),
        expires_at: start.expires_at,
    })
}

pub(crate) fn accept_requester_prepared(
    runtime: &Arc<HostRuntime>,
    prepared: NativeV2PreparedV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    if prepared.protocol_version != PROTOCOL_VERSION
        || prepared.expires_at <= now
        || prepared.bridge_id != captured.bridge_id
        || prepared.host_ref != captured.peer_host_ref
        || prepared.session_binding_ref != captured.binding_ref
        || prepared.admission_ref.trim().is_empty()
    {
        return invalid("Native v2 prepared response Host/session is invalid.");
    }
    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    let row = tx
        .query_row(
            "SELECT a.revision_id, a.revision_hash, a.approval_id, a.expires_at,
                    h.session_binding_json, h.review_correlation_id,
                    h.admission_state
             FROM native_v2_product_attempts a
             JOIN native_v2_product_hosts h ON h.attempt_id = a.attempt_id
             WHERE a.attempt_id = ?1 AND h.participant_ref = ?2 AND a.state = 'preparing'",
            params![prepared.attempt_id, prepared.participant.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 prepared response is late or unknown.".into())
        })?;
    let stored_binding: crate::host_identity::HostSessionBinding =
        serde_json::from_str(row.4.as_deref().ok_or_else(|| {
            AppError::InvalidInput("Native v2 prepared Host is unavailable.".into())
        })?)?;
    if row.0 != prepared.revision_id
        || row.1 != prepared.revision_hash
        || row.2 != prepared.approval_id
        || row.3 < prepared.expires_at
        || row.5.as_deref() != Some(prepared.correlation_id.as_str())
        || row.6 != "pending"
        || &stored_binding != captured
    {
        return invalid("Native v2 prepared response correlation is invalid or replayed.");
    }
    tx.execute(
        "UPDATE native_v2_product_hosts SET admission_state = 'prepared',
         admission_ref = ?3, updated_at = ?4
         WHERE attempt_id = ?1 AND participant_ref = ?2 AND admission_state = 'pending'",
        params![
            prepared.attempt_id,
            prepared.participant.as_str(),
            prepared.admission_ref,
            now
        ],
    )?;
    let pending: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_v2_product_hosts
         WHERE attempt_id = ?1 AND admission_state != 'prepared'",
        [prepared.attempt_id.as_str()],
        |row| row.get(0),
    )?;
    if pending != 0 {
        tx.commit()?;
        return Ok(Vec::new());
    }
    let revision_json: String = tx.query_row(
        "SELECT r.revision_json FROM native_v2_product_revisions r
         JOIN native_v2_product_attempts a ON a.revision_id = r.revision_id
         WHERE a.attempt_id = ?1",
        [prepared.attempt_id.as_str()],
        |row| row.get(0),
    )?;
    let revision: PlanRevisionV2 = serde_json::from_str(&revision_json)?;
    let mut statement = tx.prepare(
        "SELECT participant_ref, peer_route_ref, session_binding_json
         FROM native_v2_product_hosts WHERE attempt_id = ?1 AND peer_route_ref IS NOT NULL
         ORDER BY participant_ref",
    )?;
    let rows = statement
        .query_map([prepared.attempt_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut events = Vec::new();
    for (participant_ref, peer_route_ref, binding_json) in rows {
        let binding: crate::host_identity::HostSessionBinding =
            serde_json::from_str(&binding_json)?;
        let current = crate::host_runtime::current_host_session_binding(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        binding.validate_current(&current, now)?;
        let target = revision
            .participants
            .as_slice()
            .iter()
            .find(|participant| participant.participant_ref.as_str() == participant_ref)
            .ok_or_else(|| AppError::InvalidInput("Native v2 commit target vanished.".into()))?
            .participant_ref
            .clone();
        let commit = NativeV2AttemptCommitV1 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-commit-{}", uuid::Uuid::new_v4()),
            attempt_id: prepared.attempt_id.clone(),
            approval_id: prepared.approval_id.clone(),
            plan_id: prepared.plan_id.clone(),
            revision_id: prepared.revision_id.clone(),
            revision_hash: prepared.revision_hash.clone(),
            bridge_id: prepared.bridge_id.clone(),
            sender: revision.requester.clone(),
            target,
            expires_at: row.3,
        };
        let context = crate::room_control::room_control_session_context_for_peer(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        events.push(NativeV2OutboundEventV1 {
            room_id: revision.bridge_id.clone(),
            peer_route_ref,
            event: native_v2_control_event(COMMIT_KIND, serde_json::to_value(commit)?, &context)?,
        });
    }
    tx.execute(
        "UPDATE native_v2_product_hosts SET admission_state = 'committed', updated_at = ?2
         WHERE attempt_id = ?1 AND admission_state = 'prepared'",
        params![prepared.attempt_id, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_attempts SET state = 'running', updated_at = ?2
         WHERE attempt_id = ?1 AND state = 'preparing'",
        params![prepared.attempt_id, now],
    )?;
    tx.execute(
        "UPDATE native_v2_product_revisions SET state = 'running', updated_at = ?2
         WHERE revision_id = ?1 AND state = 'preparing'",
        params![prepared.revision_id, now],
    )?;
    project_eligible_product_steps(&tx, &revision, &prepared.attempt_id, now)?;
    tx.commit()?;
    Ok(events)
}

pub(crate) fn accept_receiver_commit(
    runtime: &HostRuntime,
    commit: &NativeV2AttemptCommitV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<()> {
    if commit.protocol_version != PROTOCOL_VERSION
        || commit.expires_at <= now
        || commit.bridge_id != captured.bridge_id
    {
        return invalid("Native v2 attempt commit is invalid.");
    }
    let (revision, approval) = load_receiver_attempt(&runtime.paths, &commit.attempt_id)?;
    let row = connection(&runtime.paths)?
        .query_row(
            "SELECT requester_participant_ref, target_participant_ref,
                    session_binding_json, state, expires_at
             FROM native_v2_receiver_attempts WHERE attempt_id = ?1",
            [commit.attempt_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 prepared attempt is unavailable.".into())
        })?;
    let stored_binding: crate::host_identity::HostSessionBinding = serde_json::from_str(&row.2)?;
    if commit.approval_id != approval.approval_id
        || commit.plan_id != revision.plan_id
        || commit.revision_id != revision.revision_id
        || commit.revision_hash != revision.revision_hash
        || commit.bridge_id != revision.bridge_id
        || commit.sender != revision.requester
        || commit.sender.as_str() != row.0
        || commit.target.as_str() != row.1
        || row.3 != "prepared"
        || row.4 < commit.expires_at
        || &stored_binding != captured
    {
        return invalid("Native v2 commit does not match exact prepared authority.");
    }
    let changed = connection(&runtime.paths)?.execute(
        "UPDATE native_v2_receiver_attempts SET state = 'running', updated_at = ?2
         WHERE attempt_id = ?1 AND state = 'prepared'",
        params![commit.attempt_id, now],
    )?;
    if changed != 1 {
        return invalid("Native v2 attempt commit was replayed or interrupted.");
    }
    Ok(())
}

pub(crate) fn accept_requester_step_result(
    runtime: &Arc<HostRuntime>,
    result: NativeV2StepResultV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<Vec<NativeV2OutboundEventV1>> {
    if result.protocol_version != PROTOCOL_VERSION
        || result.bridge_id != captured.bridge_id
        || result.host_ref != captured.peer_host_ref
        || result.session_binding_ref != captured.binding_ref
        || captured.local_host_ref != runtime.local_host_ref
    {
        return invalid("Native v2 step result Host/session is invalid.");
    }
    let mut conn = connection(&runtime.paths)?;
    let tx = conn.transaction()?;
    let row = tx
        .query_row(
            "SELECT a.revision_id, a.revision_hash, a.approval_id, a.expires_at,
                    r.revision_json, p.approval_json, h.session_binding_json
             FROM native_v2_product_attempts a
             JOIN native_v2_product_revisions r ON r.revision_id = a.revision_id
             JOIN native_v2_product_approvals p ON p.approval_id = a.approval_id
             JOIN native_v2_product_hosts h ON h.attempt_id = a.attempt_id
                  AND h.participant_ref = ?2
             WHERE a.attempt_id = ?1 AND a.state = 'running'",
            params![result.attempt_id, result.participant.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 step result is late or unknown.".into())
        })?;
    let revision: PlanRevisionV2 = serde_json::from_str(&row.4)?;
    let approval: PlanApprovalV2 = serde_json::from_str(&row.5)?;
    let stored_binding: crate::host_identity::HostSessionBinding =
        serde_json::from_str(row.6.as_deref().ok_or_else(|| {
            AppError::InvalidInput("Native v2 result Host is unavailable.".into())
        })?)?;
    if row.0 != result.revision_id
        || row.1 != result.revision_hash
        || row.2 != result.approval_id
        || row.3 <= now
        || &stored_binding != captured
    {
        return invalid("Native v2 result immutable correlation is invalid.");
    }
    validate_step_result(&revision, &approval, &result.attempt_id, &result)?;
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == result.step_id)
        .ok_or_else(|| AppError::InvalidInput("Native v2 result step vanished.".into()))?;
    for dependency in step.dependencies() {
        let completed: i64 = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_v2_product_steps
             WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'completed')",
            params![result.attempt_id, dependency],
            |value| value.get(0),
        )?;
        if completed != 1 {
            return invalid("Native v2 result arrived before its exact predecessor completed.");
        }
    }
    let inserted = tx.execute(
        "INSERT INTO native_v2_step_commits
         (attempt_id, step_id, revision_id, revision_hash, completion_ref, operation,
          host_ref, result_json, state, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'committed', ?9)",
        params![
            result.attempt_id,
            result.step_id,
            result.revision_id,
            result.revision_hash,
            result.completion_ref,
            operation_name_value(&result.operation),
            result.host_ref.as_str(),
            serde_json::to_string(&result)?,
            now
        ],
    );
    if inserted.is_err() {
        return invalid("Native v2 step result is duplicate, conflicting, or replayed.");
    }
    let changed = tx.execute(
        "UPDATE native_v2_product_steps SET state = 'completed', completion_ref = ?3,
         updated_at = ?4 WHERE attempt_id = ?1 AND step_id = ?2
         AND state IN ('pending','eligible','running')",
        params![
            result.attempt_id,
            result.step_id,
            result.completion_ref,
            now
        ],
    )?;
    if changed != 1 {
        return invalid("Native v2 product step completion is late or replayed.");
    }
    let remaining: i64 = tx.query_row(
        "SELECT COUNT(*) FROM native_v2_product_steps
         WHERE attempt_id = ?1 AND state != 'completed'",
        [result.attempt_id.as_str()],
        |row| row.get(0),
    )?;
    if remaining == 0 {
        tx.execute(
            "UPDATE native_v2_product_attempts SET state = 'completed', updated_at = ?2
             WHERE attempt_id = ?1 AND state = 'running'",
            params![result.attempt_id, now],
        )?;
        tx.execute(
            "UPDATE native_v2_product_revisions SET state = 'completed', updated_at = ?2
             WHERE revision_id = ?1 AND state = 'running'",
            params![result.revision_id, now],
        )?;
    } else {
        project_eligible_product_steps(&tx, &revision, &result.attempt_id, now)?;
    }
    let mut statement = tx.prepare(
        "SELECT participant_ref, peer_route_ref, session_binding_json
         FROM native_v2_product_hosts WHERE attempt_id = ?1 AND peer_route_ref IS NOT NULL
         ORDER BY participant_ref",
    )?;
    let rows = statement
        .query_map([result.attempt_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut events = Vec::new();
    for (participant_ref, peer_route_ref, binding_json) in rows {
        let binding: crate::host_identity::HostSessionBinding =
            serde_json::from_str(&binding_json)?;
        let current = crate::host_runtime::current_host_session_binding(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        binding.validate_current(&current, now)?;
        let target = revision
            .participants
            .as_slice()
            .iter()
            .find(|participant| participant.participant_ref.as_str() == participant_ref)
            .ok_or_else(|| AppError::InvalidInput("Native v2 step commit target vanished.".into()))?
            .participant_ref
            .clone();
        let commit = NativeV2StepCommitV1 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("native-v2-step-commit-{}", uuid::Uuid::new_v4()),
            attempt_id: result.attempt_id.clone(),
            approval_id: result.approval_id.clone(),
            plan_id: result.plan_id.clone(),
            revision_id: result.revision_id.clone(),
            revision_hash: result.revision_hash.clone(),
            bridge_id: result.bridge_id.clone(),
            sender: revision.requester.clone(),
            target,
            result: result.clone(),
            expires_at: row.3,
        };
        let context = crate::room_control::room_control_session_context_for_peer(
            runtime,
            &revision.bridge_id,
            &peer_route_ref,
        )?;
        events.push(NativeV2OutboundEventV1 {
            room_id: revision.bridge_id.clone(),
            peer_route_ref,
            event: native_v2_control_event(
                STEP_COMMIT_KIND,
                serde_json::to_value(commit)?,
                &context,
            )?,
        });
    }
    tx.commit()?;
    Ok(events)
}

fn project_eligible_product_steps(
    tx: &Transaction<'_>,
    revision: &PlanRevisionV2,
    attempt_id: &str,
    now: i64,
) -> AppResult<()> {
    for step in &revision.steps {
        let mut all_dependencies_completed = true;
        for dependency in step.dependencies() {
            let completed = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_v2_product_steps
                 WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'completed')",
                params![attempt_id, dependency],
                |row| row.get::<_, i64>(0),
            )?;
            if completed != 1 {
                all_dependencies_completed = false;
                break;
            }
        }
        if all_dependencies_completed {
            tx.execute(
                "UPDATE native_v2_product_steps SET state = 'eligible', updated_at = ?3
                 WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'pending'",
                params![attempt_id, step.id(), now],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn accept_receiver_step_commit(
    runtime: &HostRuntime,
    commit: &NativeV2StepCommitV1,
    captured: &crate::host_identity::HostSessionBinding,
    now: i64,
) -> AppResult<()> {
    if commit.protocol_version != PROTOCOL_VERSION
        || commit.expires_at <= now
        || commit.bridge_id != captured.bridge_id
    {
        return invalid("Native v2 step commit is invalid.");
    }
    let (revision, approval) = load_receiver_attempt(&runtime.paths, &commit.attempt_id)?;
    let row = connection(&runtime.paths)?
        .query_row(
            "SELECT requester_participant_ref, target_participant_ref,
                    session_binding_json, state, expires_at
             FROM native_v2_receiver_attempts WHERE attempt_id = ?1",
            [commit.attempt_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 running attempt is unavailable.".into())
        })?;
    let stored_binding: crate::host_identity::HostSessionBinding = serde_json::from_str(&row.2)?;
    if commit.approval_id != approval.approval_id
        || commit.plan_id != revision.plan_id
        || commit.revision_id != revision.revision_id
        || commit.revision_hash != revision.revision_hash
        || commit.sender != revision.requester
        || commit.sender.as_str() != row.0
        || commit.target.as_str() != row.1
        || row.3 != "running"
        || row.4 < commit.expires_at
        || &stored_binding != captured
    {
        return invalid("Native v2 step commit crossed prepared attempt authority.");
    }
    validate_step_result(&revision, &approval, &commit.attempt_id, &commit.result)?;
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == commit.result.step_id)
        .ok_or_else(|| AppError::InvalidInput("Native v2 committed step vanished.".into()))?;
    let conn = connection(&runtime.paths)?;
    for dependency in step.dependencies() {
        if !committed_step(&conn, &commit.attempt_id, dependency)? {
            return invalid("Native v2 step commit arrived before its exact predecessor.");
        }
    }
    if let PlanStepV2::Transfer {
        destination,
        output,
        ..
    } = step
    {
        let destination_host = participant_for_ref(&revision, destination).ok_or_else(|| {
            AppError::InvalidInput("Native v2 Transfer destination vanished.".into())
        })?;
        if destination_host.host_ref == runtime.local_host_ref {
            if !has_exact_transfer_receipt(
                &conn,
                &commit.attempt_id,
                step.id(),
                &revision,
                output,
                commit.result.content_digest.as_deref().unwrap_or_default(),
                &runtime.local_host_ref,
            )? {
                return invalid("Native v2 Transfer commit has no exact destination receipt.");
            }
        }
    }
    record_step_commit(
        &runtime.paths,
        &revision,
        &approval,
        &commit.attempt_id,
        &commit.result,
        now,
    )?;
    let complete = revision
        .steps
        .iter()
        .all(|step| committed_step(&conn, &commit.attempt_id, step.id()).unwrap_or(false));
    if complete {
        connection(&runtime.paths)?.execute(
            "UPDATE native_v2_receiver_attempts SET state = 'completed', updated_at = ?2
             WHERE attempt_id = ?1 AND state = 'running'",
            params![commit.attempt_id, now],
        )?;
    }
    Ok(())
}

fn has_exact_transfer_receipt(
    conn: &Connection,
    attempt_id: &str,
    step_id: &str,
    revision: &PlanRevisionV2,
    object: &ManagedObjectRevisionV2,
    content_digest: &str,
    destination_host_ref: &HostRef,
) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_transfer_receipts
         WHERE attempt_id = ?1 AND step_id = ?2 AND revision_id = ?3
         AND revision_hash = ?4 AND logical_object_id = ?5
         AND object_revision = ?6 AND content_digest = ?7
         AND destination_host_ref = ?8)",
        params![
            attempt_id,
            step_id,
            revision.revision_id,
            revision.revision_hash,
            object.logical_object_id,
            object.revision,
            content_digest,
            destination_host_ref.as_str()
        ],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn spawn_native_v2_events(
    runtime: Arc<HostRuntime>,
    events: Vec<NativeV2OutboundEventV1>,
) {
    if events.is_empty() {
        return;
    }
    let task_runtime = runtime.clone();
    runtime.spawn(async move {
        for event in events {
            let attempt_id = event
                .event
                .get("payload")
                .and_then(|payload| payload.get("attemptId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if send_native_v2_event(task_runtime.clone(), event)
                .await
                .is_err()
            {
                if let Some(attempt_id) = attempt_id {
                    let cancellation = terminate_requester_attempt(
                        &task_runtime,
                        &attempt_id,
                        "interrupted",
                        "coordination_delivery_failed",
                        crate::storage::now_ts(),
                    )
                    .unwrap_or_default();
                    for cancel in cancellation {
                        let _ = send_native_v2_event(task_runtime.clone(), cancel).await;
                    }
                }
                return;
            }
        }
    });
}

pub(crate) fn emit_product_status_for_attempt(runtime: &HostRuntime, attempt_id: &str) {
    let revision_id = connection(&runtime.paths).and_then(|conn| {
        conn.query_row(
            "SELECT revision_id FROM native_v2_product_attempts WHERE attempt_id = ?1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(AppError::from)
    });
    if let Ok(revision_id) = revision_id {
        if let Ok(status) =
            NativeV2ProductStore::new(&runtime.paths).status_for_revision(&revision_id)
        {
            let _ = runtime.emit(PRODUCT_STATUS_EVENT, &status);
        }
    }
}

pub(crate) fn step_result_completion_ref(result: &NativeV2StepResultV1) -> AppResult<String> {
    let mut semantic = result.clone();
    semantic.message_id.clear();
    semantic.completion_ref.clear();
    let canonical = canonical_json(&serde_json::to_value(semantic)?);
    Ok(format!(
        "native-v2-step-completion:v1:{}",
        blake3::hash(format!("native-v2-step-completion:v1\0{canonical}").as_bytes()).to_hex()
    ))
}

pub(crate) fn validate_step_result(
    revision: &PlanRevisionV2,
    approval: &PlanApprovalV2,
    expected_attempt_id: &str,
    result: &NativeV2StepResultV1,
) -> AppResult<()> {
    if result.protocol_version != PROTOCOL_VERSION
        || result.attempt_id != expected_attempt_id
        || result.approval_id != approval.approval_id
        || result.plan_id != revision.plan_id
        || result.revision_id != revision.revision_id
        || result.revision_hash != revision.revision_hash
        || result.bridge_id != revision.bridge_id
        || result.completion_ref != step_result_completion_ref(result)?
    {
        return invalid("Native v2 step result correlation is invalid.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == result.step_id)
        .ok_or_else(|| AppError::InvalidInput("Native v2 result step is unavailable.".into()))?;
    if step.operation() != result.operation {
        return invalid("Native v2 step result operation is invalid.");
    }
    let participant = participant_for_ref(revision, &result.participant)
        .ok_or_else(|| AppError::InvalidInput("Native v2 result participant is invalid.".into()))?;
    if participant.host_ref != result.host_ref || !step_completed_by(step, &result.participant) {
        return invalid("Native v2 result Host does not own the authored step completion.");
    }
    match step {
        PlanStepV2::Search { output, .. } | PlanStepV2::Transform { output, .. } => {
            if result.object.as_ref() != Some(output)
                || result.content_digest.as_deref().is_none_or(str::is_empty)
                || result.result_digest.is_some()
            {
                return invalid("Native v2 object-producing result is invalid.");
            }
        }
        PlanStepV2::Transfer { output, .. } => {
            if result.object.as_ref() != Some(output)
                || result.content_digest.as_deref().is_none_or(str::is_empty)
                || result.result_digest.is_some()
            {
                return invalid("Native v2 Transfer result is invalid.");
            }
        }
        PlanStepV2::Execute { .. } => {
            if result.object.is_some()
                || result.content_digest.is_some()
                || result.result_digest.as_deref().is_none_or(str::is_empty)
            {
                return invalid("Native v2 Execute result is invalid.");
            }
        }
    }
    Ok(())
}

pub(crate) fn step_completed_by(step: &PlanStepV2, participant: &PlanParticipantRef) -> bool {
    match step {
        PlanStepV2::Search { host, .. }
        | PlanStepV2::Transform { host, .. }
        | PlanStepV2::Execute { host, .. } => host == participant,
        PlanStepV2::Transfer { source, .. } => source == participant,
    }
}

pub(crate) fn step_runs_on_host(
    revision: &PlanRevisionV2,
    step: &PlanStepV2,
    host_ref: &HostRef,
) -> bool {
    match step {
        PlanStepV2::Search { host, .. }
        | PlanStepV2::Transform { host, .. }
        | PlanStepV2::Execute { host, .. } => participant_for_ref(revision, host)
            .is_some_and(|participant| &participant.host_ref == host_ref),
        PlanStepV2::Transfer { source, .. } => participant_for_ref(revision, source)
            .is_some_and(|participant| &participant.host_ref == host_ref),
    }
}

pub(crate) fn committed_step(
    conn: &Connection,
    attempt_id: &str,
    step_id: &str,
) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_step_commits
         WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'committed')",
        params![attempt_id, step_id],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn receiver_attempt_is_coordinated(
    conn: &Connection,
    attempt_id: &str,
) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_receiver_attempts
         WHERE attempt_id = ?1)",
        [attempt_id],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn record_step_commit(
    paths: &AppPaths,
    revision: &PlanRevisionV2,
    approval: &PlanApprovalV2,
    attempt_id: &str,
    result: &NativeV2StepResultV1,
    now: i64,
) -> AppResult<bool> {
    validate_step_result(revision, approval, attempt_id, result)?;
    let conn = connection(paths)?;
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO native_v2_step_commits
         (attempt_id, step_id, revision_id, revision_hash, completion_ref, operation,
          host_ref, result_json, state, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'committed', ?9)",
        params![
            attempt_id,
            result.step_id,
            revision.revision_id,
            revision.revision_hash,
            result.completion_ref,
            operation_name_value(&result.operation),
            result.host_ref.as_str(),
            serde_json::to_string(result)?,
            now
        ],
    )?;
    if inserted == 0 {
        let existing: String = conn.query_row(
            "SELECT completion_ref FROM native_v2_step_commits
             WHERE attempt_id = ?1 AND step_id = ?2",
            params![attempt_id, result.step_id],
            |row| row.get(0),
        )?;
        if existing != result.completion_ref {
            return invalid("Native v2 step completion conflicts with prior Core state.");
        }
    }
    Ok(inserted == 1)
}

pub(crate) fn load_receiver_attempt(
    paths: &AppPaths,
    attempt_id: &str,
) -> AppResult<(PlanRevisionV2, PlanApprovalV2)> {
    let row = connection(paths)?
        .query_row(
            "SELECT r.revision_json, p.approval_json
             FROM bridge_plan_v2_attempts a
             JOIN bridge_plan_v2_revisions r ON r.revision_id = a.revision_id
             JOIN bridge_plan_v2_approvals p ON p.approval_id = a.approval_id
             WHERE a.attempt_id = ?1 AND a.state = 'accepted'",
            [attempt_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::InvalidInput("Native v2 receiver attempt is unavailable.".into())
        })?;
    let revision: PlanRevisionV2 = serde_json::from_str(&row.0)?;
    verify_sealed_revision(&revision)?;
    Ok((revision, serde_json::from_str(&row.1)?))
}

pub(crate) fn validate_transfer_landing(
    runtime: &HostRuntime,
    metadata: &NativeV2TransferMetadataV1,
    context: &crate::room_control::RoomControlSessionContext,
    now: i64,
) -> AppResult<()> {
    if metadata.protocol_version != PROTOCOL_VERSION
        || metadata.expires_at <= now
        || metadata.bridge_id != context.room_id
        || metadata.destination_host_ref != runtime.local_host_ref
        || metadata.content_digest.trim().is_empty()
    {
        return invalid("Native v2 Transfer metadata is unavailable.");
    }
    let (revision, approval) = load_receiver_attempt(&runtime.paths, &metadata.attempt_id)?;
    if metadata.approval_id != approval.approval_id
        || metadata.plan_id != revision.plan_id
        || metadata.revision_id != revision.revision_id
        || metadata.revision_hash != revision.revision_hash
        || metadata.bridge_id != revision.bridge_id
    {
        return invalid("Native v2 Transfer crossed immutable attempt correlation.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == metadata.step_id)
        .ok_or_else(|| AppError::InvalidInput("Native v2 Transfer step is unavailable.".into()))?;
    let PlanStepV2::Transfer {
        source,
        destination,
        input,
        output,
        ..
    } = step
    else {
        return invalid("Native v2 transfer metadata names a non-Transfer step.");
    };
    let source_participant = participant_for_ref(&revision, source).ok_or_else(|| {
        AppError::InvalidInput("Native v2 Transfer source is unavailable.".into())
    })?;
    let destination_participant = participant_for_ref(&revision, destination).ok_or_else(|| {
        AppError::InvalidInput("Native v2 Transfer destination is unavailable.".into())
    })?;
    if source != &metadata.source
        || destination != &metadata.destination
        || source_participant.host_ref != metadata.source_host_ref
        || destination_participant.host_ref != metadata.destination_host_ref
        || input != &metadata.object
        || output != &metadata.object
    {
        return invalid("Native v2 Transfer metadata does not match the authored movement.");
    }
    let peer = crate::storage::list_bridge_peer_endpoints(&runtime.paths, &metadata.bridge_id)?
        .into_iter()
        .filter(|peer| peer.peer_session_id == context.peer_route_ref)
        .collect::<Vec<_>>();
    if peer.len() != 1
        || peer[0].logical_host_ref.as_deref() != Some(metadata.source_host_ref.as_str())
    {
        return invalid("Native v2 Transfer source Host/session binding is unavailable.");
    }
    let conn = connection(&runtime.paths)?;
    for dependency in step.dependencies() {
        if !committed_step(&conn, &metadata.attempt_id, dependency)? {
            return invalid("Native v2 Transfer predecessor is not Core-committed.");
        }
    }
    if committed_step(&conn, &metadata.attempt_id, &metadata.step_id)? {
        return invalid("Native v2 Transfer already completed.");
    }
    Ok(())
}

pub(crate) fn register_transfer_landing(
    runtime: &HostRuntime,
    metadata: &NativeV2TransferMetadataV1,
    path: std::path::PathBuf,
    media_type: String,
    size_bytes: u64,
    now: i64,
) -> AppResult<()> {
    let scope_root = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| AppError::InvalidInput("Native v2 transfer root is unavailable.".into()))?;
    let display_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("managed-input")
        .to_string();
    let acquisition = runtime.managed_objects.lock().bind_transferred_revision(
        crate::managed_objects::HostArtifactAcquisition {
            kind: crate::managed_objects::ManagedObjectAcquisitionKind::TransferReceipt,
            source_ref: format!(
                "native-v2-transfer:{}:{}",
                metadata.attempt_id, metadata.step_id
            ),
            bridge_id: Some(metadata.bridge_id.clone()),
            path,
            scope_root,
            display_name,
            media_type,
            expires_at: metadata.expires_at,
            app_owned_temporary: true,
        },
        metadata.object.logical_object_id.clone(),
        metadata.object.revision,
        metadata.content_digest.clone(),
        now,
    )?;
    if acquisition.object.size_bytes != size_bytes
        || acquisition.object.host_ref != metadata.destination_host_ref
    {
        return invalid("Native v2 Transfer receipt content is invalid.");
    }
    connection(&runtime.paths)?.execute(
        "INSERT INTO native_v2_transfer_receipts
         (attempt_id, step_id, revision_id, revision_hash, logical_object_id,
          object_revision, content_digest, destination_host_ref, binding_ref, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            metadata.attempt_id,
            metadata.step_id,
            metadata.revision_id,
            metadata.revision_hash,
            metadata.object.logical_object_id,
            metadata.object.revision,
            metadata.content_digest,
            metadata.destination_host_ref.as_str(),
            acquisition.binding.binding_ref,
            now
        ],
    )?;
    Ok(())
}

pub(crate) fn init_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS native_v2_product_revisions (
            revision_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, bridge_id TEXT NOT NULL,
            revision_hash TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL CHECK(state IN
                ('draft','approved','checking_readiness','preparing','running','completed',
                 'failed','interrupted','cancelled')),
            revision_json TEXT NOT NULL, failure_code TEXT,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS native_v2_product_approvals (
            approval_id TEXT PRIMARY KEY, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('valid','revoked','expired','burned')),
            approval_json TEXT NOT NULL, created_at INTEGER NOT NULL,
            FOREIGN KEY(revision_id) REFERENCES native_v2_product_revisions(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS native_v2_product_attempts (
            attempt_id TEXT PRIMARY KEY, approval_id TEXT NOT NULL, revision_id TEXT NOT NULL,
            revision_hash TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN
                ('checking_readiness','preparing','running','completed','failed','interrupted','cancelled')),
            failure_code TEXT, expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            FOREIGN KEY(approval_id) REFERENCES native_v2_product_approvals(approval_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS native_v2_product_hosts (
            attempt_id TEXT NOT NULL, participant_ref TEXT NOT NULL, host_ref TEXT NOT NULL,
            peer_route_ref TEXT, session_binding_ref TEXT, session_binding_json TEXT,
            review_correlation_id TEXT, review_request_nonce TEXT, review_json TEXT,
            start_json TEXT, readiness_state TEXT NOT NULL CHECK(readiness_state IN
                ('pending','ready','unavailable')),
            admission_state TEXT NOT NULL CHECK(admission_state IN
                ('pending','prepared','committed','failed','cancelled')),
            readiness_code TEXT, admission_ref TEXT, updated_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, participant_ref),
            FOREIGN KEY(attempt_id) REFERENCES native_v2_product_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS native_v2_product_steps (
            revision_id TEXT NOT NULL, attempt_id TEXT, step_id TEXT NOT NULL,
            operation TEXT NOT NULL CHECK(operation IN ('search','transform','transfer','execute')),
            state TEXT NOT NULL CHECK(state IN
                ('pending','eligible','running','completed','failed','interrupted','cancelled')),
            completion_ref TEXT, updated_at INTEGER NOT NULL,
            UNIQUE(revision_id, attempt_id, step_id)
        );
        CREATE TABLE IF NOT EXISTS native_v2_receiver_attempts (
            attempt_id TEXT PRIMARY KEY, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            approval_id TEXT NOT NULL, requester_participant_ref TEXT NOT NULL,
            target_participant_ref TEXT NOT NULL, session_binding_ref TEXT NOT NULL,
            session_binding_json TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN
                ('reviewed','prepared','running','completed','failed','interrupted','cancelled')),
            failure_code TEXT, expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS native_v2_receiver_reviews (
            correlation_id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL UNIQUE,
            revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            approval_id TEXT NOT NULL, requester_participant_ref TEXT NOT NULL,
            target_participant_ref TEXT NOT NULL, session_binding_json TEXT NOT NULL,
            readiness_state TEXT NOT NULL CHECK(readiness_state IN ('ready','unavailable')),
            readiness_code TEXT, expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS native_v2_step_commits (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            completion_ref TEXT NOT NULL UNIQUE,
            operation TEXT NOT NULL CHECK(operation IN ('search','transform','transfer','execute')),
            host_ref TEXT NOT NULL, result_json TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('committed','interrupted')),
            committed_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id)
        );
        CREATE TABLE IF NOT EXISTS native_v2_transfer_receipts (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
            logical_object_id TEXT NOT NULL, object_revision INTEGER NOT NULL,
            content_digest TEXT NOT NULL, destination_host_ref TEXT NOT NULL,
            binding_ref TEXT NOT NULL UNIQUE, received_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id)
        );
        CREATE TABLE IF NOT EXISTS native_v2_external_dispatches (
            attempt_id TEXT NOT NULL, step_id TEXT NOT NULL,
            operation TEXT NOT NULL CHECK(operation IN ('search','transfer')),
            state TEXT NOT NULL CHECK(state IN
                ('dispatching','completed','failed','interrupted','cancelled')),
            transfer_id TEXT, failure_code TEXT,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id)
        );
        CREATE TRIGGER IF NOT EXISTS native_v2_product_revision_immutable
        BEFORE UPDATE OF plan_id, bridge_id, revision_hash, revision_json, created_at
        ON native_v2_product_revisions
        BEGIN SELECT RAISE(ABORT, 'Native v2 product revision is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS native_v2_product_attempt_authority_immutable
        BEFORE UPDATE OF approval_id, revision_id, revision_hash, expires_at, created_at
        ON native_v2_product_attempts
        BEGIN SELECT RAISE(ABORT, 'Native v2 product attempt authority is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS native_v2_receiver_attempt_authority_immutable
        BEFORE UPDATE OF revision_id, revision_hash, approval_id, requester_participant_ref,
                         target_participant_ref, session_binding_ref, session_binding_json,
                         expires_at, created_at
        ON native_v2_receiver_attempts
        BEGIN SELECT RAISE(ABORT, 'Native v2 receiver attempt authority is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS native_v2_step_commit_immutable
        BEFORE UPDATE ON native_v2_step_commits
        BEGIN SELECT RAISE(ABORT, 'Native v2 step completion is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS native_v2_external_dispatch_identity_immutable
        BEFORE UPDATE OF attempt_id, step_id, operation, created_at
        ON native_v2_external_dispatches
        BEGIN SELECT RAISE(ABORT, 'Native v2 external dispatch identity is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS native_v2_external_dispatch_state_guard
        BEFORE UPDATE OF state ON native_v2_external_dispatches
        WHEN NOT (OLD.state = 'dispatching' AND NEW.state IN
            ('completed','failed','interrupted','cancelled'))
        BEGIN SELECT RAISE(ABORT, 'Illegal native v2 external dispatch transition'); END;
        "#,
    )?;
    Ok(())
}

pub(crate) fn delete_bridge_records(tx: &Transaction<'_>, bridge_id: &str) -> AppResult<()> {
    // Product revisions use a separate table family from the protocol-plan
    // revisions below. Their un-FK'd steps and receiver records must be
    // deleted explicitly so a burned Bridge cannot retain replayable results.
    tx.execute(
        "DELETE FROM native_v2_external_dispatches WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_product_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)
          UNION SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_transfer_receipts WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_product_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)
          UNION SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_step_commits WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_product_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)
          UNION SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_receiver_reviews WHERE revision_id IN
         (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_receiver_attempts WHERE revision_id IN
         (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_product_steps WHERE revision_id IN
         (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_external_dispatches WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_transfer_receipts WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_step_commits WHERE attempt_id IN
         (SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
          (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1))",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_receiver_reviews WHERE revision_id IN
         (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_receiver_attempts WHERE revision_id IN
         (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1)",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM native_v2_product_revisions WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    Ok(())
}

pub(crate) fn reconcile_startup(paths: &AppPaths, now: i64) -> AppResult<usize> {
    let conn = connection(paths)?;
    let requester = conn.execute(
        "UPDATE native_v2_product_attempts SET state = 'interrupted',
         failure_code = 'host_restarted', updated_at = ?1
         WHERE state IN ('checking_readiness','preparing','running')",
        [now],
    )?;
    conn.execute(
        "UPDATE native_v2_product_revisions SET state = 'interrupted',
         failure_code = 'host_restarted', updated_at = ?1
         WHERE state IN ('checking_readiness','preparing','running')",
        [now],
    )?;
    let receiver = conn.execute(
        "UPDATE native_v2_receiver_attempts SET state = 'interrupted',
         failure_code = 'host_restarted', updated_at = ?1
         WHERE state IN ('reviewed','prepared','running')",
        [now],
    )?;
    Ok(requester + receiver)
}

pub(crate) fn interrupt_attempts_for_bridge(
    paths: &AppPaths,
    bridge_id: &str,
    code: &str,
    now: i64,
) -> usize {
    interrupt_attempts(paths, Some(bridge_id), None, code, now).unwrap_or_default()
}

pub(crate) fn interrupt_attempts_for_session(
    paths: &AppPaths,
    session_binding_ref: &str,
    code: &str,
    now: i64,
) -> usize {
    interrupt_attempts(paths, None, Some(session_binding_ref), code, now).unwrap_or_default()
}

pub(crate) fn interrupt_all_attempts(paths: &AppPaths, code: &str, now: i64) -> usize {
    interrupt_attempts(paths, None, None, code, now).unwrap_or_default()
}

fn interrupt_attempts(
    paths: &AppPaths,
    bridge_id: Option<&str>,
    session_binding_ref: Option<&str>,
    code: &str,
    now: i64,
) -> AppResult<usize> {
    text(code, "interruption code")?;
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let attempt_ids = if let Some(bridge_id) = bridge_id {
        let mut statement = tx.prepare(
            "SELECT attempt_id FROM native_v2_product_attempts WHERE revision_id IN
             (SELECT revision_id FROM native_v2_product_revisions WHERE bridge_id = ?1)
             UNION SELECT attempt_id FROM native_v2_receiver_attempts WHERE revision_id IN
             (SELECT revision_id FROM bridge_plan_v2_revisions WHERE bridge_id = ?1)",
        )?;
        let values = statement
            .query_map([bridge_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        values
    } else if let Some(binding_ref) = session_binding_ref {
        let mut statement = tx.prepare(
            "SELECT attempt_id FROM native_v2_product_hosts WHERE session_binding_ref = ?1
             UNION SELECT attempt_id FROM native_v2_receiver_attempts
             WHERE session_binding_ref = ?1",
        )?;
        let values = statement
            .query_map([binding_ref], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        values
    } else {
        let mut statement = tx.prepare(
            "SELECT attempt_id FROM native_v2_product_attempts
             UNION SELECT attempt_id FROM native_v2_receiver_attempts",
        )?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        values
    };
    let mut interrupted = 0;
    for attempt_id in attempt_ids {
        interrupted += tx.execute(
            "UPDATE native_v2_product_attempts SET state = 'interrupted', failure_code = ?2,
             updated_at = ?3 WHERE attempt_id = ?1
             AND state IN ('checking_readiness','preparing','running')",
            params![attempt_id, code, now],
        )?;
        tx.execute(
            "UPDATE native_v2_product_revisions SET state = 'interrupted', failure_code = ?2,
             updated_at = ?3 WHERE revision_id IN
             (SELECT revision_id FROM native_v2_product_attempts WHERE attempt_id = ?1)
             AND state IN ('checking_readiness','preparing','running')",
            params![attempt_id, code, now],
        )?;
        interrupted += tx.execute(
            "UPDATE native_v2_receiver_attempts SET state = 'interrupted', failure_code = ?2,
             updated_at = ?3 WHERE attempt_id = ?1
             AND state IN ('reviewed','prepared','running')",
            params![attempt_id, code, now],
        )?;
        tx.execute(
            "UPDATE native_v2_external_dispatches SET state = 'interrupted', failure_code = ?2,
             updated_at = ?3 WHERE attempt_id = ?1 AND state = 'dispatching'",
            params![attempt_id, code, now],
        )?;
        tx.execute(
            "UPDATE native_v2_product_steps SET state = 'interrupted', updated_at = ?2
             WHERE attempt_id = ?1 AND state IN ('pending','eligible','running')",
            params![attempt_id, now],
        )?;
    }
    tx.commit()?;
    Ok(interrupted)
}

fn operation_name(step: &PlanStepV2) -> &'static str {
    operation_name_value(&step.operation())
}

fn operation_name_value(operation: &StepOperation) -> &'static str {
    match operation {
        StepOperation::Search => "search",
        StepOperation::Transform => "transform",
        StepOperation::Transfer => "transfer",
        StepOperation::Execute => "execute",
    }
}

fn parse_product_state(value: &str) -> AppResult<NativeV2ProductStateV1> {
    match value {
        "draft" => Ok(NativeV2ProductStateV1::Draft),
        "approved" => Ok(NativeV2ProductStateV1::Approved),
        "checking_readiness" => Ok(NativeV2ProductStateV1::CheckingReadiness),
        "preparing" => Ok(NativeV2ProductStateV1::Preparing),
        "running" => Ok(NativeV2ProductStateV1::Running),
        "completed" => Ok(NativeV2ProductStateV1::Completed),
        "failed" => Ok(NativeV2ProductStateV1::Failed),
        "interrupted" => Ok(NativeV2ProductStateV1::Interrupted),
        "cancelled" => Ok(NativeV2ProductStateV1::Cancelled),
        _ => invalid("Native v2 product state is invalid."),
    }
}

fn ensure_active_bridge(paths: &AppPaths, bridge_id: &str) -> AppResult<()> {
    let available: i64 = connection(paths)?.query_row(
        "SELECT EXISTS(SELECT 1 FROM rooms WHERE id = ?1 AND status = 'active')
         AND NOT EXISTS(SELECT 1 FROM burned_bridges WHERE room_id = ?1)",
        [bridge_id],
        |row| row.get(0),
    )?;
    if available != 1 {
        return invalid("Native v2 product Bridge is unavailable or burned.");
    }
    Ok(())
}

fn connection(paths: &AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn id(value: &str, label: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_ID
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
    {
        return invalid(&format!("Native v2 {label} is invalid."));
    }
    Ok(())
}

fn text(value: &str, label: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        return invalid(&format!("Native v2 {label} is invalid."));
    }
    Ok(())
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string serialization"),
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
                        serde_json::to_string(key).expect("key serialization"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::StoredConfig,
        host_runtime::{HostEvent, HostEventSink, RuntimeTask, RuntimeTaskSpawner},
        models::LocalRole,
        storage,
    };

    const NOW: i64 = 10_000;

    struct NoopEventSink;

    impl HostEventSink for NoopEventSink {
        fn emit(&self, _event: HostEvent) -> AppResult<()> {
            Ok(())
        }
    }

    struct NoopTaskSpawner;

    impl RuntimeTaskSpawner for NoopTaskSpawner {
        fn spawn(&self, _task: RuntimeTask) {}
    }

    fn host(value: &str) -> HostRef {
        HostRef::from_device_id(value).unwrap()
    }

    fn compose(hosts: [&HostRef; 3]) -> NativeV2ComposeRequestV1 {
        NativeV2ComposeRequestV1 {
            plan_id: "plan-native-v2".into(),
            revision_id: "revision-native-v2".into(),
            revision_number: 1,
            bridge_id: "bridge-native-v2".into(),
            requester_host_ref: hosts[0].as_str().into(),
            participant_host_refs: hosts.iter().map(|host| host.as_str().into()).collect(),
            roots: vec![NativeV2RootDraftV1 {
                root_id: "root-project".into(),
                object: NativeV2ObjectRevisionDtoV1 {
                    logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                    revision: 1,
                },
                host_ref: hosts[1].as_str().into(),
            }],
            original_user_goal: "Transform on B, move explicitly, execute on C.".into(),
            expected_outcome: "One exact transferred revision is executed on C.".into(),
            steps: vec![
                NativeV2StepDraftV1::Transform {
                    step_id: "transform-b".into(),
                    depends_on: vec![],
                    host_ref: hosts[1].as_str().into(),
                    input: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                        revision: 1,
                    },
                    output: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                        revision: 2,
                    },
                    modification_intent: "Apply the approved semantic change.".into(),
                },
                NativeV2StepDraftV1::Transfer {
                    step_id: "transfer-b-c".into(),
                    depends_on: vec!["transform-b".into()],
                    source_host_ref: hosts[1].as_str().into(),
                    destination_host_ref: hosts[2].as_str().into(),
                    input: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                        revision: 2,
                    },
                    output: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                        revision: 2,
                    },
                },
                NativeV2StepDraftV1::Execute {
                    step_id: "execute-c".into(),
                    depends_on: vec!["transfer-b-c".into()],
                    host_ref: hosts[2].as_str().into(),
                    target: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
                        revision: 2,
                    },
                    execution_intent: "Run the exact configured validation.".into(),
                },
            ],
        }
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
            Some("bridge-native-v2".into()),
            Some(NOW + 3_600),
        )
        .unwrap();
        paths
    }

    fn requester_runtime(paths: AppPaths, device_id: &str) -> Arc<HostRuntime> {
        Arc::new(
            HostRuntime::new(
                paths,
                StoredConfig {
                    version: 5,
                    default_expiry_minutes: 15,
                    inbox_dir: None,
                    auto_burn_after_download: false,
                    save_received_files_to_inbox: true,
                    save_received_images_to_inbox: true,
                    transfer_window_override: None,
                    dev_tools_enabled: false,
                    micro_flow_group_mode: "off".into(),
                    shortcut: "test".into(),
                    app_secret: crate::crypto::encode_key(&[19u8; 32]),
                    device_id: device_id.into(),
                },
                Arc::new(NoopEventSink),
                Arc::new(NoopTaskSpawner),
            )
            .unwrap(),
        )
    }

    fn seed_running_product(paths: &AppPaths, revision: &PlanRevisionV2) {
        let store = NativeV2ProductStore::new(paths);
        let requester = crate::bridge_plan_v2::requester_host(revision).unwrap();
        store.create_draft(revision, requester, NOW).unwrap();
        let approval = store
            .approve("revision-native-v2", "approval-native-v2", NOW + 1_000, NOW)
            .unwrap();
        let conn = connection(paths).unwrap();
        conn.execute(
            "INSERT INTO native_v2_product_attempts
             (attempt_id, approval_id, revision_id, revision_hash, state, failure_code,
              expires_at, created_at, updated_at)
             VALUES ('attempt-native-v2', ?1, ?2, ?3, 'running', NULL, ?4, ?5, ?5)",
            params![
                approval.approval_id,
                revision.revision_id,
                revision.revision_hash,
                NOW + 1_000,
                NOW
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE native_v2_product_revisions SET state = 'running' WHERE revision_id = ?1",
            [revision.revision_id.as_str()],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM native_v2_product_steps WHERE revision_id = ?1",
            [revision.revision_id.as_str()],
        )
        .unwrap();
        for step in &revision.steps {
            conn.execute(
                "INSERT INTO native_v2_product_steps
                 (revision_id, attempt_id, step_id, operation, state, completion_ref, updated_at)
                 VALUES (?1, 'attempt-native-v2', ?2, ?3, 'pending', NULL, ?4)",
                params![revision.revision_id, step.id(), operation_name(step), NOW],
            )
            .unwrap();
        }
    }

    #[test]
    fn deterministic_composition_and_hash_are_stable_across_participant_order() {
        let a = host("a");
        let b = host("b");
        let c = host("c");
        let first = compose_revision(compose([&a, &b, &c])).unwrap();
        let mut reordered = compose([&a, &b, &c]);
        reordered.participant_host_refs.reverse();
        let second = compose_revision(reordered).unwrap();
        assert_eq!(first, second);
        assert!(first
            .revision_hash
            .starts_with("bridge-plan-revision-hash-v2:"));
    }

    #[test]
    fn host_root_and_topology_substitution_change_or_invalidate_the_revision() {
        let a = host("a");
        let b = host("b");
        let c = host("c");
        let first = compose_revision(compose([&a, &b, &c])).unwrap();
        let mut substituted = compose([&a, &b, &c]);
        substituted.roots[0].host_ref = c.as_str().into();
        assert!(compose_revision(substituted).is_err());
        let mut revision_substituted = compose([&a, &b, &c]);
        revision_substituted.revision_id = "different-revision".into();
        let second = compose_revision(revision_substituted).unwrap();
        assert_ne!(first.revision_hash, second.revision_hash);
    }

    #[test]
    fn no_implicit_movement_or_inserted_step_can_validate() {
        let a = host("a");
        let b = host("b");
        let c = host("c");
        let mut request = compose([&a, &b, &c]);
        request.steps.remove(1);
        if let NativeV2StepDraftV1::Execute { depends_on, .. } = &mut request.steps[1] {
            *depends_on = vec!["transform-b".into()];
        }
        assert!(compose_revision(request).is_err());
    }

    #[test]
    fn exact_completion_unlocks_only_the_authored_dependency_chain() {
        let paths = paths("native-v2-dependency-projection");
        let revision = compose_revision(compose([&host("a"), &host("b"), &host("c")])).unwrap();
        seed_running_product(&paths, &revision);
        let mut conn = connection(&paths).unwrap();
        let tx = conn.transaction().unwrap();
        project_eligible_product_steps(&tx, &revision, "attempt-native-v2", NOW).unwrap();
        tx.commit().unwrap();
        let state = |step: &str| {
            connection(&paths)
                .unwrap()
                .query_row(
                    "SELECT state FROM native_v2_product_steps
                     WHERE attempt_id = 'attempt-native-v2' AND step_id = ?1",
                    [step],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        assert_eq!(state("transform-b"), "eligible");
        assert_eq!(state("transfer-b-c"), "pending");
        assert_eq!(state("execute-c"), "pending");

        connection(&paths)
            .unwrap()
            .execute(
                "UPDATE native_v2_product_steps SET state = 'completed'
                 WHERE attempt_id = 'attempt-native-v2' AND step_id = 'transform-b'",
                [],
            )
            .unwrap();
        let mut conn = connection(&paths).unwrap();
        let tx = conn.transaction().unwrap();
        project_eligible_product_steps(&tx, &revision, "attempt-native-v2", NOW + 1).unwrap();
        tx.commit().unwrap();
        assert_eq!(state("transfer-b-c"), "eligible");
        assert_eq!(state("execute-c"), "pending");
    }

    #[test]
    fn one_receivers_local_readiness_rejection_fails_the_whole_plan_barrier() {
        let paths = paths("native-v2-whole-plan-readiness-rejection");
        let runtime = requester_runtime(paths.clone(), "a");
        let requester_host = host("a");
        let transform_host = host("b");
        let execute_host = host("c");
        let revision =
            compose_revision(compose([&requester_host, &transform_host, &execute_host])).unwrap();
        let store = NativeV2ProductStore::new(&paths);
        store.create_draft(&revision, &requester_host, NOW).unwrap();
        let approval = store
            .approve(
                &revision.revision_id,
                "approval-readiness",
                NOW + 1_000,
                NOW,
            )
            .unwrap();
        let transform_participant = participant_for_ref(
            &revision,
            &PlanParticipantRef::for_host(&revision.plan_id, &transform_host).unwrap(),
        )
        .unwrap()
        .participant_ref
        .clone();
        let execute_participant = participant_for_ref(
            &revision,
            &PlanParticipantRef::for_host(&revision.plan_id, &execute_host).unwrap(),
        )
        .unwrap()
        .participant_ref
        .clone();
        let transform_binding = crate::host_identity::HostSessionBinding::new(
            &revision.bridge_id,
            requester_host.clone(),
            transform_host.clone(),
            "requester-session-b",
            "receiver-session-b",
            "route-b",
            NOW + 1_000,
        )
        .unwrap();
        let execute_binding = crate::host_identity::HostSessionBinding::new(
            &revision.bridge_id,
            requester_host.clone(),
            execute_host.clone(),
            "requester-session-c",
            "receiver-session-c",
            "route-c",
            NOW + 1_000,
        )
        .unwrap();
        let conn = connection(&paths).unwrap();
        conn.execute(
            "INSERT INTO native_v2_product_attempts
             (attempt_id, approval_id, revision_id, revision_hash, state, failure_code,
              expires_at, created_at, updated_at)
             VALUES ('attempt-readiness', ?1, ?2, ?3, 'checking_readiness', NULL,
                     ?4, ?5, ?5)",
            params![
                approval.approval_id,
                revision.revision_id,
                revision.revision_hash,
                NOW + 1_000,
                NOW
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE native_v2_product_revisions SET state = 'checking_readiness',
             updated_at = ?2 WHERE revision_id = ?1",
            params![revision.revision_id, NOW],
        )
        .unwrap();
        for (participant, host_ref, route, binding, correlation) in [
            (
                &transform_participant,
                &transform_host,
                "route-b",
                &transform_binding,
                "correlation-b",
            ),
            (
                &execute_participant,
                &execute_host,
                "route-c",
                &execute_binding,
                "correlation-c",
            ),
        ] {
            conn.execute(
                "INSERT INTO native_v2_product_hosts
                 (attempt_id, participant_ref, host_ref, peer_route_ref, session_binding_ref,
                  session_binding_json, review_correlation_id, readiness_state,
                  admission_state, updated_at)
                 VALUES ('attempt-readiness', ?1, ?2, ?3, ?4, ?5, ?6,
                         'pending', 'pending', ?7)",
                params![
                    participant.as_str(),
                    host_ref.as_str(),
                    route,
                    binding.binding_ref,
                    serde_json::to_string(binding).unwrap(),
                    correlation,
                    NOW
                ],
            )
            .unwrap();
        }
        drop(conn);

        let events = accept_requester_readiness(
            &runtime,
            NativeV2ReadinessV1 {
                protocol_version: PROTOCOL_VERSION.into(),
                message_id: "readiness-result-b".into(),
                correlation_id: "correlation-b".into(),
                attempt_id: "attempt-readiness".into(),
                approval_id: approval.approval_id,
                plan_id: revision.plan_id.clone(),
                revision_id: revision.revision_id.clone(),
                revision_hash: revision.revision_hash.clone(),
                bridge_id: revision.bridge_id.clone(),
                participant: transform_participant.clone(),
                host_ref: transform_host,
                session_binding_ref: transform_binding.binding_ref.clone(),
                ready: false,
                code: Some("managed_platform_unavailable".into()),
                expires_at: NOW + 500,
            },
            &transform_binding,
            NOW + 1,
        )
        .unwrap();
        assert!(events.is_empty());
        let attempt: (String, String) = connection(&paths)
            .unwrap()
            .query_row(
                "SELECT state, failure_code FROM native_v2_product_attempts
                 WHERE attempt_id = 'attempt-readiness'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(attempt, ("failed".into(), "whole_plan_unavailable".into()));
        let host_states: Vec<(String, String)> = connection(&paths)
            .unwrap()
            .prepare(
                "SELECT participant_ref, readiness_state FROM native_v2_product_hosts
                 WHERE attempt_id = 'attempt-readiness' ORDER BY participant_ref",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            host_states.contains(&(transform_participant.as_str().into(), "unavailable".into()))
        );
        assert!(host_states.contains(&(execute_participant.as_str().into(), "pending".into())));
        drop(runtime);
        std::fs::remove_dir_all(paths.app_data_dir).unwrap();
    }

    #[test]
    fn session_revocation_interrupts_attempt_and_rejects_late_continuation_state() {
        let paths = paths("native-v2-session-revocation");
        let revision = compose_revision(compose([&host("a"), &host("b"), &host("c")])).unwrap();
        seed_running_product(&paths, &revision);
        let participant = revision.participants.as_slice()[1].clone();
        connection(&paths)
            .unwrap()
            .execute(
                "INSERT INTO native_v2_product_hosts
                 (attempt_id, participant_ref, host_ref, peer_route_ref, session_binding_ref,
                  session_binding_json, readiness_state, admission_state, updated_at)
                 VALUES ('attempt-native-v2', ?1, ?2, 'peer-b', 'binding-b', '{}',
                         'ready', 'committed', ?3)",
                params![
                    participant.participant_ref.as_str(),
                    participant.host_ref.as_str(),
                    NOW
                ],
            )
            .unwrap();
        assert_eq!(
            interrupt_attempts_for_session(&paths, "binding-b", "session_revoked", NOW + 1),
            1
        );
        let row: (String, String) = connection(&paths)
            .unwrap()
            .query_row(
                "SELECT state, failure_code FROM native_v2_product_attempts
                 WHERE attempt_id = 'attempt-native-v2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("interrupted".into(), "session_revoked".into()));
        assert_eq!(
            connection(&paths)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM native_v2_product_steps
                     WHERE attempt_id = 'attempt-native-v2' AND state = 'eligible'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn managed_failure_or_revocation_makes_the_coordinated_receiver_terminal() {
        let paths = paths("native-v2-managed-receiver-terminal");
        let local = host("b");
        let requester = host("a");
        let binding = crate::host_identity::HostSessionBinding::new(
            "bridge-native-v2",
            local,
            requester,
            "session-b",
            "session-a",
            "peer-a",
            NOW + 1_000,
        )
        .unwrap();
        connection(&paths)
            .unwrap()
            .execute(
                "INSERT INTO native_v2_receiver_attempts
                 (attempt_id, revision_id, revision_hash, approval_id,
                  requester_participant_ref, target_participant_ref,
                  session_binding_ref, session_binding_json, state, failure_code,
                  expires_at, created_at, updated_at)
                 VALUES ('attempt-revoked', 'revision-native-v2', 'hash-native-v2',
                         'approval-native-v2', 'participant-a', 'participant-b', ?1, ?2,
                         'running', NULL, ?3, ?4, ?4)",
                params![
                    binding.binding_ref,
                    serde_json::to_string(&binding).unwrap(),
                    NOW + 1_000,
                    NOW
                ],
            )
            .unwrap();

        terminate_receiver_managed_attempt(
            &paths,
            "attempt-revoked",
            "interrupted",
            "provider_revoked",
            NOW + 1,
        )
        .unwrap();
        let row: (String, String) = connection(&paths)
            .unwrap()
            .query_row(
                "SELECT state, failure_code FROM native_v2_receiver_attempts
                 WHERE attempt_id = 'attempt-revoked'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("interrupted".into(), "provider_revoked".into()));
        assert_eq!(
            receiver_attempt_binding(&paths, "attempt-revoked").unwrap(),
            Some(binding)
        );
        assert!(terminate_receiver_managed_attempt(
            &paths,
            "attempt-revoked",
            "completed",
            "provider_revoked",
            NOW + 2,
        )
        .is_err());
    }

    #[test]
    fn search_transfer_only_plan_has_no_provider_or_implicit_managed_step() {
        let a = host("a");
        let b = host("b");
        let c = host("c");
        let object = NativeV2ObjectRevisionDtoV1 {
            logical_object_id: format!("managed-object:v1:{}", "b".repeat(64)),
            revision: 1,
        };
        let request = NativeV2ComposeRequestV1 {
            plan_id: "search-transfer-plan".into(),
            revision_id: "search-transfer-revision".into(),
            revision_number: 1,
            bridge_id: "bridge-native-v2".into(),
            requester_host_ref: a.as_str().into(),
            participant_host_refs: vec![a.as_str().into(), b.as_str().into(), c.as_str().into()],
            roots: Vec::new(),
            original_user_goal: "Find and explicitly transfer one object.".into(),
            expected_outcome: "The exact revision arrives at C.".into(),
            steps: vec![
                NativeV2StepDraftV1::Search {
                    step_id: "search-b".into(),
                    depends_on: Vec::new(),
                    host_ref: b.as_str().into(),
                    output: object.clone(),
                    query: "report.txt".into(),
                    safe_scope_labels: vec!["documents".into()],
                },
                NativeV2StepDraftV1::Transfer {
                    step_id: "transfer-b-c".into(),
                    depends_on: vec!["search-b".into()],
                    source_host_ref: b.as_str().into(),
                    destination_host_ref: c.as_str().into(),
                    input: object.clone(),
                    output: object,
                },
            ],
        };
        let revision = compose_revision(request).unwrap();
        assert!(revision.steps.iter().all(|step| matches!(
            step,
            PlanStepV2::Search { .. } | PlanStepV2::Transfer { .. }
        )));
    }

    #[test]
    fn requester_local_primitives_fail_readiness_until_self_admission_exists() {
        let a = host("a");
        let b = host("b");
        let c = host("c");
        let object = NativeV2ObjectRevisionDtoV1 {
            logical_object_id: format!("managed-object:v1:{}", "c".repeat(64)),
            revision: 1,
        };
        let revision = compose_revision(NativeV2ComposeRequestV1 {
            plan_id: "requester-local-plan".into(),
            revision_id: "requester-local-revision".into(),
            revision_number: 1,
            bridge_id: "bridge-native-v2".into(),
            requester_host_ref: a.as_str().into(),
            participant_host_refs: vec![a.as_str().into(), b.as_str().into()],
            roots: Vec::new(),
            original_user_goal: "Find locally, then transfer explicitly.".into(),
            expected_outcome: "The exact revision arrives at B.".into(),
            steps: vec![
                NativeV2StepDraftV1::Search {
                    step_id: "search-a".into(),
                    depends_on: Vec::new(),
                    host_ref: a.as_str().into(),
                    output: object.clone(),
                    query: "report.txt".into(),
                    safe_scope_labels: vec!["documents".into()],
                },
                NativeV2StepDraftV1::Transfer {
                    step_id: "transfer-a-b".into(),
                    depends_on: vec!["search-a".into()],
                    source_host_ref: a.as_str().into(),
                    destination_host_ref: b.as_str().into(),
                    input: object.clone(),
                    output: object,
                },
            ],
        })
        .unwrap();
        assert!(requester_has_local_step(&revision, &revision.requester, &a));
        assert!(!requester_has_local_step(
            &revision,
            &revision.requester,
            &c
        ));
    }

    #[test]
    fn destination_commit_requires_the_exact_same_revision_transfer_receipt() {
        let paths = paths("native-v2-transfer-receipt");
        let revision = compose_revision(compose([&host("a"), &host("b"), &host("c")])).unwrap();
        let PlanStepV2::Transfer {
            step_id,
            output,
            destination,
            ..
        } = &revision.steps[1]
        else {
            panic!("expected Transfer");
        };
        let destination_host = participant_for_ref(&revision, destination).unwrap();
        let conn = connection(&paths).unwrap();
        assert!(!has_exact_transfer_receipt(
            &conn,
            "attempt-native-v2",
            step_id,
            &revision,
            output,
            "digest-exact",
            &destination_host.host_ref,
        )
        .unwrap());
        conn.execute(
            "INSERT INTO native_v2_transfer_receipts
             (attempt_id, step_id, revision_id, revision_hash, logical_object_id,
              object_revision, content_digest, destination_host_ref, binding_ref, received_at)
             VALUES ('attempt-native-v2', ?1, ?2, ?3, ?4, ?5, 'digest-exact', ?6,
                     'binding-receipt', ?7)",
            params![
                step_id,
                revision.revision_id,
                revision.revision_hash,
                output.logical_object_id,
                output.revision,
                destination_host.host_ref.as_str(),
                NOW
            ],
        )
        .unwrap();
        assert!(has_exact_transfer_receipt(
            &conn,
            "attempt-native-v2",
            step_id,
            &revision,
            output,
            "digest-exact",
            &destination_host.host_ref,
        )
        .unwrap());
        let wrong_revision = ManagedObjectRevisionV2 {
            logical_object_id: output.logical_object_id.clone(),
            revision: output.revision + 1,
        };
        assert!(!has_exact_transfer_receipt(
            &conn,
            "attempt-native-v2",
            step_id,
            &revision,
            &wrong_revision,
            "digest-exact",
            &destination_host.host_ref,
        )
        .unwrap());
    }

    #[test]
    fn burn_deletes_product_revision_steps_and_replay_records() {
        let paths = paths("native-v2-burn-cleanup");
        let revision = compose_revision(compose([&host("a"), &host("b"), &host("c")])).unwrap();
        seed_running_product(&paths, &revision);
        let step_id = "burn-extra".to_string();
        let conn = connection(&paths).unwrap();
        conn.execute(
            "INSERT INTO native_v2_product_steps
             (revision_id, attempt_id, step_id, operation, state, completion_ref, updated_at)
             VALUES (?1, 'attempt-native-v2', ?2, 'transfer', 'completed', 'completion-product', ?3)",
            params![revision.revision_id, step_id, NOW],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_v2_receiver_attempts
             (attempt_id, revision_id, revision_hash, approval_id, requester_participant_ref,
              target_participant_ref, session_binding_ref, session_binding_json, state,
              failure_code, expires_at, created_at, updated_at)
             VALUES ('receiver-product', ?1, ?2, 'approval-native-v2', 'participant-a',
                     'participant-b', 'binding-product', '{}', 'completed', NULL, ?3, ?4, ?4)",
            params![
                revision.revision_id,
                revision.revision_hash,
                NOW + 1_000,
                NOW
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_v2_receiver_reviews
             (correlation_id, attempt_id, revision_id, revision_hash, approval_id,
              requester_participant_ref, target_participant_ref, session_binding_json,
              readiness_state, readiness_code, expires_at, created_at)
             VALUES ('correlation-product', 'receiver-product', ?1, ?2, 'approval-native-v2',
                     'participant-a', 'participant-b', '{}', 'ready', NULL, ?3, ?4)",
            params![
                revision.revision_id,
                revision.revision_hash,
                NOW + 1_000,
                NOW
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_v2_transfer_receipts
             (attempt_id, step_id, revision_id, revision_hash, logical_object_id,
              object_revision, content_digest, destination_host_ref, binding_ref, received_at)
             VALUES ('receiver-product', ?1, ?2, ?3, 'managed-object:v1:receipt', 1,
                     'digest-product', 'host-b', 'binding-product-receipt', ?4)",
            params![step_id, revision.revision_id, revision.revision_hash, NOW],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_v2_step_commits
             (attempt_id, step_id, revision_id, revision_hash, completion_ref, operation,
              host_ref, result_json, state, committed_at)
             VALUES ('receiver-product', ?1, ?2, ?3, 'commit-product', 'transfer',
                     'host-b', '{}', 'committed', ?4)",
            params![step_id, revision.revision_id, revision.revision_hash, NOW],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO native_v2_external_dispatches
             (attempt_id, step_id, operation, state, transfer_id, failure_code, created_at, updated_at)
             VALUES ('receiver-product', ?1, 'transfer', 'completed', 'transfer-product', NULL, ?2, ?2)",
            params![step_id, NOW],
        )
        .unwrap();
        drop(conn);

        storage::cut_off_bridge_authority(&paths, "bridge-native-v2").unwrap();
        crate::bridge_plan::delete_bridge_records(&paths, "bridge-native-v2").unwrap();

        let conn = connection(&paths).unwrap();
        for table in [
            "native_v2_product_revisions",
            "native_v2_product_steps",
            "native_v2_receiver_attempts",
            "native_v2_receiver_reviews",
            "native_v2_transfer_receipts",
            "native_v2_step_commits",
            "native_v2_external_dispatches",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "Burn retained rows in {table}");
        }
        let _ = std::fs::remove_dir_all(paths.app_data_dir);
    }

    #[test]
    fn orchestration_has_no_worker_effect_network_terminal_or_provider_authority_types() {
        let source = include_str!("native_v2_orchestration.rs");
        for forbidden in [
            concat!("Effect", "RequestV1"),
            concat!("Network", "GrantV1"),
            concat!("Developer", "TerminalGrant"),
            concat!("Worker", "SecretHandle"),
        ] {
            assert!(
                !source.contains(forbidden),
                "unexpected authority type: {forbidden}"
            );
        }
    }
}

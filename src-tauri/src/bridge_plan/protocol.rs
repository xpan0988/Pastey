//! Versioned, fixed-shape Room Control messages for Bridge Plan review.
//!
//! The receiver-side grant below is deliberately process-local.  It contains
//! no ObjectRef, path, token, worker, or transport material and is cleared on
//! restart and Burn.

use std::{collections::HashMap, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{Map, Value};

use super::{
    canonical_json, canonical_revision_hash, connection, id, json, ActivityKind, AttemptState,
    BridgePlanActivity, BridgePlanApproval, BridgePlanAttempt, BridgePlanResultSummary,
    BridgePlanRevision, BridgePlanStep, BridgePlanStore, GeneratedUserVisibleText,
    ReceiverDecision, ReceiverDecisionEvidence, SafeActivitySummary, StepExecutionState,
};
use crate::{
    error::{AppError, AppResult},
    file_candidates::{BridgePlanPrivateFile, BridgePlanSearchResult},
    storage::AppPaths,
};

pub(crate) const PROTOCOL_VERSION: &str = "pastey-bridge-plan-protocol-v1";
const MAX_LIFETIME: i64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub(crate) struct ProtocolMetadata {
    pub(crate) replay_id: String,
}

#[derive(Clone, Debug)]
struct LocalSearchAuthority {
    bridge_id: String,
    attempt_id: String,
    step_id: String,
    expires_at: i64,
}
#[derive(Clone, Debug)]
struct LocalTransferAuthority {
    bridge_id: String,
    attempt_id: String,
    step_id: String,
    expires_at: i64,
}
#[derive(Clone, Debug)]
struct LocalTransformAuthority {
    bridge_id: String,
    attempt_id: String,
    step_id: String,
    expires_at: i64,
}
#[derive(Clone, Debug)]
struct LocalTransformOutput {
    bridge_id: String,
    output: BridgePlanPrivateFile,
}
#[derive(Clone, Debug)]
struct LocalCandidateSelection {
    bridge_id: String,
    attempt_id: String,
    candidate_id: String,
    expires_at: i64,
}

/// Receiver-local execution data derived only after an authenticated
/// attempt-start message matches the receiver's prior review. It is never
/// serialized, never sent over room control, and is consumed before Search.
#[derive(Clone, Debug)]
pub(crate) struct SearchExecutionGrant {
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) requester_device_ref: String,
    pub(crate) receiver_device_ref: String,
    pub(crate) approval_id: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) search_step_digest: String,
    pub(crate) filename_hint: String,
    pub(crate) extensions: Vec<String>,
    pub(crate) safe_scope_labels: Vec<String>,
}

/// Receiver-local authority for the Transfer step that follows a bounded
/// Search selection. The selected file remains in the private candidate store;
/// this grant contains only the immutable plan binding and no resolver data.
#[derive(Clone, Debug)]
pub(crate) struct TransferExecutionGrant {
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) requester_device_ref: String,
    pub(crate) receiver_device_ref: String,
    pub(crate) approval_id: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) step_digest: String,
    pub(crate) candidate_id: String,
    pub(crate) generated_file: Option<BridgePlanPrivateFile>,
    pub(crate) destination: super::TransferDestination,
}

/// Receiver-local authority for a Transform following a bounded Search
/// selection. The selected source remains in the private candidate store and
/// generated output remains in the local ephemeral object store.
#[derive(Clone, Debug)]
pub(crate) struct TransformExecutionGrant {
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) requester_device_ref: String,
    pub(crate) receiver_device_ref: String,
    pub(crate) approval_id: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) step_digest: String,
    pub(crate) candidate_id: String,
    pub(crate) intent: String,
}

/// Candidate data allowed to cross the Bridge Plan result boundary. It has no
/// ObjectRef, local path, source request, lease, or authority material.
#[derive(Clone, Debug)]
pub(crate) struct SafeSearchCandidate {
    pub(crate) candidate_id: String,
    pub(crate) display_name: String,
    pub(crate) redacted_location: String,
    pub(crate) extension: String,
    pub(crate) mime_family: String,
    pub(crate) size_bytes: u64,
    pub(crate) modified_at: String,
    pub(crate) match_reason: String,
    pub(crate) confidence: String,
}

#[derive(Default)]
pub(crate) struct ProtocolSearchAuthorityStore {
    grants: Mutex<HashMap<String, LocalSearchAuthority>>,
    transfer_grants: Mutex<HashMap<String, LocalTransferAuthority>>,
    transform_grants: Mutex<HashMap<String, LocalTransformAuthority>>,
    transform_outputs: Mutex<HashMap<String, LocalTransformOutput>>,
    selections: Mutex<HashMap<String, LocalCandidateSelection>>,
}

impl ProtocolSearchAuthorityStore {
    fn grant(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        step_id: &str,
        expires_at: i64,
    ) -> AppResult<()> {
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        if grants.contains_key(attempt_id) {
            return invalid("Bridge Plan attempt was already started.");
        }
        grants.insert(
            attempt_id.into(),
            LocalSearchAuthority {
                bridge_id: bridge_id.into(),
                attempt_id: attempt_id.into(),
                step_id: step_id.into(),
                expires_at,
            },
        );
        Ok(())
    }
    fn revoke(&self, attempt_id: &str) {
        if let Ok(mut grants) = self.grants.lock() {
            grants.remove(attempt_id);
        }
    }
    fn revoke_transfer(&self, attempt_id: &str) {
        if let Ok(mut grants) = self.transfer_grants.lock() {
            grants.remove(attempt_id);
        }
    }
    fn revoke_transform(&self, attempt_id: &str) {
        if let Ok(mut grants) = self.transform_grants.lock() {
            grants.remove(attempt_id);
        }
    }
    fn grant_transfer(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        step_id: &str,
        expires_at: i64,
    ) -> AppResult<()> {
        let mut grants = self
            .transfer_grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        if grants.contains_key(attempt_id) {
            return invalid("Bridge Plan Transfer was already started.");
        }
        grants.insert(
            attempt_id.into(),
            LocalTransferAuthority {
                bridge_id: bridge_id.into(),
                attempt_id: attempt_id.into(),
                step_id: step_id.into(),
                expires_at,
            },
        );
        Ok(())
    }
    fn consume_transfer(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<LocalTransferAuthority> {
        let mut grants = self
            .transfer_grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        let grant = grants.remove(attempt_id).ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transfer authority is unavailable.".into())
        })?;
        if grant.bridge_id != bridge_id || grant.expires_at <= now {
            return invalid("Bridge Plan Transfer authority expired or crossed Bridge scope.");
        }
        Ok(grant)
    }
    fn grant_transform(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        step_id: &str,
        expires_at: i64,
    ) -> AppResult<()> {
        let mut grants = self
            .transform_grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        if grants.contains_key(attempt_id) {
            return invalid("Bridge Plan Transform was already started.");
        }
        grants.insert(
            attempt_id.into(),
            LocalTransformAuthority {
                bridge_id: bridge_id.into(),
                attempt_id: attempt_id.into(),
                step_id: step_id.into(),
                expires_at,
            },
        );
        Ok(())
    }
    fn consume_transform(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<LocalTransformAuthority> {
        let mut grants = self
            .transform_grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        let grant = grants.remove(attempt_id).ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transform authority is unavailable.".into())
        })?;
        if grant.bridge_id != bridge_id || grant.expires_at <= now {
            return invalid("Bridge Plan Transform authority expired or crossed Bridge scope.");
        }
        Ok(grant)
    }
    fn consume(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<LocalSearchAuthority> {
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| AppError::InvalidInput("Bridge Plan authority unavailable.".into()))?;
        let grant = grants.remove(attempt_id).ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Search authority is unavailable.".into())
        })?;
        if grant.bridge_id != bridge_id || grant.expires_at <= now {
            return invalid("Bridge Plan Search authority expired or crossed Bridge scope.");
        }
        Ok(grant)
    }
    pub(crate) fn purge_bridge(&self, bridge_id: &str) {
        if let Ok(mut grants) = self.grants.lock() {
            grants.retain(|_, value| value.bridge_id != bridge_id);
        }
        if let Ok(mut grants) = self.transfer_grants.lock() {
            grants.retain(|_, value| value.bridge_id != bridge_id);
        }
        if let Ok(mut grants) = self.transform_grants.lock() {
            grants.retain(|_, value| value.bridge_id != bridge_id);
        }
        if let Ok(mut selections) = self.selections.lock() {
            selections.retain(|_, value| value.bridge_id != bridge_id);
        }
        if let Ok(mut outputs) = self.transform_outputs.lock() {
            outputs.retain(|_, value| value.bridge_id != bridge_id);
        }
    }
    fn bind_selection(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        candidate_id: &str,
        expires_at: i64,
    ) -> AppResult<()> {
        let mut selections = self.selections.lock().map_err(|_| {
            AppError::InvalidInput("Bridge Plan selection authority unavailable.".into())
        })?;
        if selections.contains_key(attempt_id) {
            return invalid("Bridge Plan candidate was already selected for this attempt.");
        }
        selections.insert(
            attempt_id.into(),
            LocalCandidateSelection {
                bridge_id: bridge_id.into(),
                attempt_id: attempt_id.into(),
                candidate_id: candidate_id.into(),
                expires_at,
            },
        );
        Ok(())
    }
    pub(crate) fn selected_candidate_id(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<String> {
        let selections = self.selections.lock().map_err(|_| {
            AppError::InvalidInput("Bridge Plan selection authority unavailable.".into())
        })?;
        let selection = selections.get(attempt_id).ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan candidate selection is unavailable.".into())
        })?;
        if selection.bridge_id != bridge_id
            || selection.attempt_id != attempt_id
            || selection.expires_at <= now
        {
            return invalid("Bridge Plan candidate selection expired or crossed Bridge scope.");
        }
        Ok(selection.candidate_id.clone())
    }
    fn consume_selected_candidate_id(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        now: i64,
    ) -> AppResult<String> {
        let mut selections = self.selections.lock().map_err(|_| {
            AppError::InvalidInput("Bridge Plan selection authority unavailable.".into())
        })?;
        let selection = selections.remove(attempt_id).ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan candidate selection is unavailable.".into())
        })?;
        if selection.bridge_id != bridge_id
            || selection.attempt_id != attempt_id
            || selection.expires_at <= now
        {
            return invalid("Bridge Plan candidate selection expired or crossed Bridge scope.");
        }
        Ok(selection.candidate_id)
    }
    pub(crate) fn retain_transform_output(
        &self,
        bridge_id: &str,
        attempt_id: &str,
        output: BridgePlanPrivateFile,
    ) -> AppResult<()> {
        let mut outputs = self.transform_outputs.lock().map_err(|_| {
            AppError::InvalidInput("Bridge Plan Transform output authority unavailable.".into())
        })?;
        if outputs
            .insert(
                attempt_id.into(),
                LocalTransformOutput {
                    bridge_id: bridge_id.into(),
                    output,
                },
            )
            .is_some()
        {
            return invalid("Bridge Plan Transform result was already retained.");
        }
        Ok(())
    }
    fn consume_transform_output(
        &self,
        bridge_id: &str,
        attempt_id: &str,
    ) -> AppResult<BridgePlanPrivateFile> {
        let output = self
            .transform_outputs
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Bridge Plan Transform output authority unavailable.".into())
            })?
            .remove(attempt_id)
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Bridge Plan Transform result is unavailable after restart or expiry.".into(),
                )
            })?;
        if output.bridge_id != bridge_id {
            return invalid("Bridge Plan Transform result crossed Bridge scope.");
        }
        Ok(output.output)
    }
    fn has_transform_output(&self, bridge_id: &str, attempt_id: &str) -> AppResult<bool> {
        Ok(self
            .transform_outputs
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Bridge Plan Transform output authority unavailable.".into())
            })?
            .get(attempt_id)
            .is_some_and(|output| output.bridge_id == bridge_id))
    }
}

pub(crate) fn review_request_payload(
    approval: &BridgePlanApproval,
    revision: &BridgePlanRevision,
) -> AppResult<Value> {
    let search = revision
        .steps
        .iter()
        .find(|step| matches!(step, BridgePlanStep::Search { .. }))
        .ok_or_else(|| {
            AppError::InvalidInput(
                "Bridge Plan currently requires a Search step for receiver review.".into(),
            )
        })?;
    let mut payload = Map::new();
    payload.insert(
        "schemaVersion".into(),
        Value::String(PROTOCOL_VERSION.into()),
    );
    payload.insert("bridgeId".into(), Value::String(approval.bridge_id.clone()));
    payload.insert("planId".into(), Value::String(approval.plan_id.clone()));
    payload.insert(
        "revisionId".into(),
        Value::String(approval.revision_id.clone()),
    );
    payload.insert(
        "revisionHash".into(),
        Value::String(approval.revision_hash.clone()),
    );
    payload.insert(
        "requesterDeviceRef".into(),
        Value::String(approval.requester_device_ref.clone()),
    );
    payload.insert(
        "receiverDeviceRef".into(),
        Value::String(approval.selected_device_ref.clone()),
    );
    payload.insert(
        "approvalId".into(),
        Value::String(approval.approval_id.clone()),
    );
    payload.insert(
        "correlationId".into(),
        Value::String(format!("review-{}", uuid::Uuid::new_v4())),
    );
    payload.insert(
        "requestNonce".into(),
        Value::String(format!("review-nonce-{}", uuid::Uuid::new_v4())),
    );
    payload.insert(
        "reviewExpiresAt".into(),
        Value::Number(approval.expires_at.into()),
    );
    payload.insert("revision".into(), serde_json::to_value(revision)?);
    payload.insert("searchStep".into(), serde_json::to_value(search)?);
    payload.insert(
        "searchStepDigest".into(),
        Value::String(step_digest(search)?),
    );
    Ok(Value::Object(payload))
}

pub(crate) fn receiver_decision_payload(
    paths: &AppPaths,
    bridge_id: &str,
    approval_id: &str,
    allow: bool,
    now: i64,
) -> AppResult<Value> {
    let conn = connection(paths)?;
    let stored = conn.query_row(
        "SELECT plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,correlation_id,request_nonce,search_step_digest,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",
        params![bridge_id, approval_id],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,String>(7)?,row.get::<_,i64>(8)?,row.get::<_,Option<String>>(9)?)),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan review is unavailable.".into()))?;
    if stored.8 <= now || stored.9.is_some() {
        return invalid("Bridge Plan review is no longer available.");
    }
    let mut payload = Map::new();
    payload.insert(
        "schemaVersion".into(),
        Value::String(PROTOCOL_VERSION.into()),
    );
    payload.insert("bridgeId".into(), Value::String(bridge_id.into()));
    payload.insert("planId".into(), Value::String(stored.0));
    payload.insert("revisionId".into(), Value::String(stored.1));
    payload.insert("revisionHash".into(), Value::String(stored.2));
    payload.insert("requesterDeviceRef".into(), Value::String(stored.3));
    payload.insert("receiverDeviceRef".into(), Value::String(stored.4));
    payload.insert("approvalId".into(), Value::String(approval_id.into()));
    payload.insert("correlationId".into(), Value::String(stored.5));
    payload.insert("requestNonce".into(), Value::String(stored.6));
    payload.insert("reviewExpiresAt".into(), Value::Number(stored.8.into()));
    payload.insert("searchStepDigest".into(), Value::String(stored.7));
    payload.insert(
        "decision".into(),
        Value::String(if allow { "allow" } else { "deny" }.into()),
    );
    payload.insert("reviewedAt".into(), Value::Number(now.into()));
    payload.insert(
        "attestationVersion".into(),
        Value::String(PROTOCOL_VERSION.into()),
    );
    let digest = attestation_digest(&payload)?;
    payload.insert("attestationDigest".into(), Value::String(digest));
    Ok(Value::Object(payload))
}

/// Safe receiver-local projection for restoring product review UI after a
/// renderer reload. It exposes only the receiver's decision, never the review
/// payload, attestation, or execution authority.
pub(crate) fn receiver_review_decision(
    paths: &AppPaths,
    bridge_id: &str,
    approval_id: &str,
) -> AppResult<Option<ReceiverDecision>> {
    let conn = connection(paths)?;
    let decision = conn.query_row(
        "SELECT decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",
        params![bridge_id, approval_id],
        |row| row.get::<_, Option<String>>(0),
    ).optional()?.flatten();
    match decision.as_deref() {
        None => Ok(None),
        Some("allow") => Ok(Some(ReceiverDecision::Approved)),
        Some("deny") => Ok(Some(ReceiverDecision::Denied)),
        Some(_) => invalid("Bridge Plan receiver review has an invalid decision."),
    }
}

pub(crate) fn attempt_start_payload(
    paths: &AppPaths,
    attempt: &BridgePlanAttempt,
    now: i64,
) -> AppResult<Value> {
    let conn = connection(paths)?;
    let revision_json: String = conn.query_row(
        "SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='requester' AND approval_id=?2",
        params![attempt.bridge_id, attempt.approval_id], |row| row.get(0),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan receiver review request was not delivered.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let search = revision
        .steps
        .iter()
        .find(|step| matches!(step, BridgePlanStep::Search { .. }))
        .ok_or_else(|| {
            AppError::InvalidInput(
                "Bridge Plan currently requires a Search step for execution.".into(),
            )
        })?;
    let mut payload = Map::new();
    payload.insert(
        "schemaVersion".into(),
        Value::String(PROTOCOL_VERSION.into()),
    );
    payload.insert("bridgeId".into(), Value::String(attempt.bridge_id.clone()));
    payload.insert("planId".into(), Value::String(attempt.plan_id.clone()));
    payload.insert(
        "revisionId".into(),
        Value::String(attempt.revision_id.clone()),
    );
    payload.insert(
        "revisionHash".into(),
        Value::String(attempt.revision_hash.clone()),
    );
    payload.insert(
        "requesterDeviceRef".into(),
        Value::String(revision.requesting_device_ref),
    );
    payload.insert(
        "receiverDeviceRef".into(),
        Value::String(revision.selected_device_ref),
    );
    payload.insert(
        "approvalId".into(),
        Value::String(attempt.approval_id.clone()),
    );
    payload.insert(
        "attemptId".into(),
        Value::String(attempt.attempt_id.clone()),
    );
    payload.insert("searchStep".into(), serde_json::to_value(search)?);
    payload.insert(
        "searchStepDigest".into(),
        Value::String(step_digest(search)?),
    );
    payload.insert(
        "attemptNonce".into(),
        Value::String(format!("attempt-nonce-{}", uuid::Uuid::new_v4())),
    );
    payload.insert(
        "attemptExpiresAt".into(),
        Value::Number((now + MAX_LIFETIME).into()),
    );
    Ok(Value::Object(payload))
}

pub(crate) fn search_selection_payload(
    paths: &AppPaths,
    bridge_id: &str,
    attempt_id: &str,
    candidate_id: &str,
) -> AppResult<Value> {
    if candidate_id.is_empty()
        || candidate_id.len() > 128
        || candidate_id.starts_with('/')
        || candidate_id.contains('/')
        || candidate_id.contains('\\')
    {
        return invalid("Bridge Plan candidate selection is invalid.");
    }
    let conn = connection(paths)?;
    let stored=conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,search_step_digest,state FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2",params![bridge_id,attempt_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan attempt is unavailable.".into()))?;
    if stored.8 != "completed" {
        return invalid("Bridge Plan Search is not ready for selection.");
    }
    Ok(
        serde_json::json!({"schemaVersion":PROTOCOL_VERSION,"bridgeId":bridge_id,"planId":stored.1,"revisionId":stored.2,"revisionHash":stored.3,"requesterDeviceRef":stored.4,"receiverDeviceRef":stored.5,"approvalId":stored.0,"attemptId":attempt_id,"stepId":stored.6,"searchStepDigest":stored.7,"candidateId":candidate_id}),
    )
}

/// Starts the already-approved Transfer step after the requester has selected
/// one candidate from the bounded Search result. The selected candidate itself
/// never crosses this boundary again: the receiver validates and resolves it
/// from its private candidate store.
pub(crate) fn transfer_start_payload(
    paths: &AppPaths,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<Value> {
    let conn = connection(paths)?;
    let stored=conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,state FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2",params![bridge_id,attempt_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).optional()?;
    let Some(stored) = stored else {
        return direct_transfer_start_payload(paths, bridge_id, attempt_id, now);
    };
    if stored.6 != "completed" {
        return invalid("Bridge Plan Search must finish before Transfer can start.");
    }
    let revision_json:String=conn.query_row("SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='requester' AND approval_id=?2",params![bridge_id,stored.0],|r|r.get(0)).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan review record is unavailable.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let transfer = revision
        .steps
        .iter()
        .find(|step| matches!(step, BridgePlanStep::Transfer { .. }))
        .ok_or_else(|| {
            AppError::InvalidInput("This plan has no supported Transfer step.".into())
        })?;
    let BridgePlanStep::Transfer { destination, .. } = transfer else {
        return invalid("This plan has no supported Transfer step.");
    };
    let supported_destination = supported_transfer_destination(destination, &stored.4, &stored.5);
    if !supported_destination {
        return invalid("Bridge Plan Transfer destination crossed its device binding.");
    }
    Ok(
        serde_json::json!({"schemaVersion":PROTOCOL_VERSION,"bridgeId":bridge_id,"planId":stored.1,"revisionId":stored.2,"revisionHash":stored.3,"requesterDeviceRef":stored.4,"receiverDeviceRef":stored.5,"approvalId":stored.0,"attemptId":attempt_id,"transferStep":transfer,"transferStepDigest":step_digest(transfer)?,"attemptNonce":format!("transfer-nonce-{}",uuid::Uuid::new_v4()),"attemptExpiresAt":now+MAX_LIFETIME}),
    )
}

fn direct_transfer_start_payload(
    paths: &AppPaths,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<Value> {
    let store = BridgePlanStore::new(paths);
    let attempt = store.list_attempt(attempt_id)?;
    if attempt.attempt.bridge_id != bridge_id || attempt.state != AttemptState::Running {
        return invalid("Bridge Plan direct Transfer is unavailable.");
    }
    let revision = store.get_revision(&attempt.attempt.revision_id)?.revision;
    let transfer = revision.steps.iter().find(|step| matches!(step, BridgePlanStep::Transfer { .. }))
        .ok_or_else(|| AppError::InvalidInput("This plan has no supported Transfer step.".into()))?;
    let eligible = attempt.steps.iter().find(|step| step.step_id == transfer.id())
        .is_some_and(|step| matches!(step.state, StepExecutionState::Eligible | StepExecutionState::Authorized));
    if !eligible {
        return invalid("Bridge Plan direct Transfer is not ready to start.");
    }
    let BridgePlanStep::Transfer { source, destination, .. } = transfer else { unreachable!() };
    if !matches!(source, super::ObjectSelectionRule::FutureUserSelection { .. })
        || !matches!(destination, super::TransferDestination::SelectedDevice { device_ref } if device_ref == &revision.selected_device_ref)
    {
        return invalid("This direct Transfer binding is unavailable.");
    }
    Ok(serde_json::json!({"schemaVersion":PROTOCOL_VERSION,"bridgeId":bridge_id,"planId":attempt.attempt.plan_id,"revisionId":attempt.attempt.revision_id,"revisionHash":attempt.attempt.revision_hash,"requesterDeviceRef":revision.requesting_device_ref,"receiverDeviceRef":revision.selected_device_ref,"approvalId":attempt.attempt.approval_id,"attemptId":attempt_id,"transferStep":transfer,"transferStepDigest":step_digest(transfer)?,"attemptNonce":format!("transfer-nonce-{}",uuid::Uuid::new_v4()),"attemptExpiresAt":now+MAX_LIFETIME}))
}

/// The live file product exposes only reviewed destinations that can execute
/// without serializing a local path: encrypted delivery to either bound
/// Bridge device, or a copy into the selected device's Pastey Shared root.
/// Keep this binding shared by dispatch and grant consumption so neither side
/// can accept a broader destination than the other.
fn supported_transfer_destination(
    destination: &super::TransferDestination,
    requester_device_ref: &str,
    receiver_device_ref: &str,
) -> bool {
    match destination {
        super::TransferDestination::RequestingDevice { device_ref } => {
            device_ref == requester_device_ref
        }
        super::TransferDestination::UserSelectedLocation {
            device_ref,
            user_visible_location_scope,
        } => {
            device_ref == receiver_device_ref
                && user_visible_location_scope.as_str() == "Pastey Shared"
        }
        super::TransferDestination::SelectedDevice { device_ref } => device_ref == receiver_device_ref,
        _ => false,
    }
}

pub(crate) fn transform_start_payload(
    paths: &AppPaths,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<Value> {
    let conn = connection(paths)?;
    let stored = conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,state FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2", params![bridge_id,attempt_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan attempt is unavailable.".into()))?;
    if stored.6 != "completed" {
        return invalid("Bridge Plan Search must finish before Transform can start.");
    }
    let revision_json: String = conn.query_row("SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='requester' AND approval_id=?2", params![bridge_id,stored.0], |r| r.get(0)).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan review record is unavailable.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let transform = revision
        .steps
        .iter()
        .find(|step| matches!(step, BridgePlanStep::Transform { .. }))
        .ok_or_else(|| {
            AppError::InvalidInput("This plan has no supported Transform step.".into())
        })?;
    Ok(
        serde_json::json!({"schemaVersion":PROTOCOL_VERSION,"bridgeId":bridge_id,"planId":stored.1,"revisionId":stored.2,"revisionHash":stored.3,"requesterDeviceRef":stored.4,"receiverDeviceRef":stored.5,"approvalId":stored.0,"attemptId":attempt_id,"transformStep":transform,"transformStepDigest":step_digest(transform)?,"attemptNonce":format!("transform-nonce-{}",uuid::Uuid::new_v4()),"attemptExpiresAt":now+MAX_LIFETIME}),
    )
}

pub(crate) fn consume_search_execution_grant(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<SearchExecutionGrant> {
    let local = authorities.consume(bridge_id, attempt_id, now)?;
    let conn = connection(paths)?;
    let stored = conn.query_row(
        "SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,search_step_digest FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND state='accepted'",
        params![bridge_id, attempt_id],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,String>(7)?)),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan Search attempt is unavailable.".into()))?;
    if local.step_id != stored.6 {
        return invalid("Bridge Plan local authority does not match the attempt.");
    }
    let revision_json: String = conn.query_row(
        "SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",
        params![bridge_id, stored.0], |row| row.get(0),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan reviewed revision is unavailable.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let search = revision
        .steps
        .iter()
        .find(|step| step.id() == stored.6)
        .ok_or_else(|| AppError::InvalidInput("Bridge Plan Search step is unavailable.".into()))?;
    let BridgePlanStep::Search { query, .. } = search else {
        return invalid("Bridge Plan execution step is not Search.");
    };
    Ok(SearchExecutionGrant {
        bridge_id: bridge_id.into(),
        plan_id: stored.1,
        revision_id: stored.2,
        revision_hash: stored.3,
        requester_device_ref: stored.4,
        receiver_device_ref: stored.5,
        approval_id: stored.0,
        attempt_id: attempt_id.into(),
        step_id: stored.6,
        search_step_digest: stored.7,
        filename_hint: query.query.as_str().into(),
        extensions: query
            .extensions
            .iter()
            .map(|extension| extension.as_str().into())
            .collect(),
        safe_scope_labels: query
            .safe_scope_labels
            .iter()
            .map(|scope| scope.as_str().into())
            .collect(),
    })
}

pub(crate) fn consume_transfer_execution_grant(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<TransferExecutionGrant> {
    let local = authorities.consume_transfer(bridge_id, attempt_id, now)?;
    let conn = connection(paths)?;
    let stored=conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest FROM bridge_plan_protocol_transfer_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND state='accepted'",params![bridge_id,attempt_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan Transfer is unavailable.".into()))?;
    if local.step_id != stored.6 {
        return invalid("Bridge Plan Transfer authority does not match the attempt.");
    }
    let revision_json:String=conn.query_row("SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",params![bridge_id,stored.0],|r|r.get(0)).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan reviewed revision is unavailable.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let transfer = revision
        .steps
        .iter()
        .find(|step| step.id() == stored.6)
        .ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transfer step is unavailable.".into())
        })?;
    let BridgePlanStep::Transfer {
        source,
        destination,
        ..
    } = transfer
    else {
        return invalid("This Transfer destination is not available yet.");
    };
    let supported_destination = supported_transfer_destination(destination, &stored.4, &stored.5);
    if !supported_destination || step_digest(transfer)? != stored.7 {
        return invalid("Bridge Plan Transfer binding mismatch.");
    }
    let generated_file = match source {
        super::ObjectSelectionRule::FromSlot { slot_id } if slot_id == "transformed_file" => {
            Some(authorities.consume_transform_output(bridge_id, attempt_id)?)
        }
        super::ObjectSelectionRule::FromSlot { slot_id } if slot_id == "selected_file" => None,
        _ => return invalid("Bridge Plan Transfer source is not available yet."),
    };
    let candidate_id = if generated_file.is_none() {
        authorities.consume_selected_candidate_id(bridge_id, attempt_id, now)?
    } else {
        String::new()
    };
    Ok(TransferExecutionGrant {
        bridge_id: bridge_id.into(),
        plan_id: stored.1,
        revision_id: stored.2,
        revision_hash: stored.3,
        requester_device_ref: stored.4,
        receiver_device_ref: stored.5,
        approval_id: stored.0,
        attempt_id: attempt_id.into(),
        step_id: stored.6,
        step_digest: stored.7,
        candidate_id,
        generated_file,
        destination: destination.clone(),
    })
}

pub(crate) fn consume_transform_execution_grant(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    bridge_id: &str,
    attempt_id: &str,
    now: i64,
) -> AppResult<TransformExecutionGrant> {
    let local = authorities.consume_transform(bridge_id, attempt_id, now)?;
    let conn = connection(paths)?;
    let stored = conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest FROM bridge_plan_protocol_transform_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND state='accepted'", params![bridge_id,attempt_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?))).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan Transform is unavailable.".into()))?;
    if local.step_id != stored.6 {
        return invalid("Bridge Plan Transform authority does not match the attempt.");
    }
    let revision_json: String = conn.query_row("SELECT revision_json FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2", params![bridge_id,stored.0], |r| r.get(0)).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan reviewed revision is unavailable.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_str(&revision_json)?;
    let transform = revision
        .steps
        .iter()
        .find(|step| step.id() == stored.6)
        .ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transform step is unavailable.".into())
        })?;
    let BridgePlanStep::Transform { intent, .. } = transform else {
        return invalid("Bridge Plan execution step is not Transform.");
    };
    if step_digest(transform)? != stored.7 {
        return invalid("Bridge Plan Transform binding mismatch.");
    }
    let candidate_id = authorities.consume_selected_candidate_id(bridge_id, attempt_id, now)?;
    Ok(TransformExecutionGrant {
        bridge_id: bridge_id.into(),
        plan_id: stored.1,
        revision_id: stored.2,
        revision_hash: stored.3,
        requester_device_ref: stored.4,
        receiver_device_ref: stored.5,
        approval_id: stored.0,
        attempt_id: attempt_id.into(),
        step_id: stored.6,
        step_digest: stored.7,
        candidate_id,
        intent: intent.as_str().into(),
    })
}

pub(crate) fn attempt_update_payload(
    grant: &SearchExecutionGrant,
    kind: &str,
    safe_summary: Option<&str>,
    failure_code: Option<&str>,
) -> AppResult<Value> {
    let mut payload = Map::new();
    payload.insert(
        "schemaVersion".into(),
        Value::String(PROTOCOL_VERSION.into()),
    );
    payload.insert("bridgeId".into(), Value::String(grant.bridge_id.clone()));
    payload.insert("planId".into(), Value::String(grant.plan_id.clone()));
    payload.insert(
        "revisionId".into(),
        Value::String(grant.revision_id.clone()),
    );
    payload.insert(
        "revisionHash".into(),
        Value::String(grant.revision_hash.clone()),
    );
    payload.insert(
        "requesterDeviceRef".into(),
        Value::String(grant.requester_device_ref.clone()),
    );
    payload.insert(
        "receiverDeviceRef".into(),
        Value::String(grant.receiver_device_ref.clone()),
    );
    payload.insert(
        "approvalId".into(),
        Value::String(grant.approval_id.clone()),
    );
    payload.insert("attemptId".into(), Value::String(grant.attempt_id.clone()));
    payload.insert("stepId".into(), Value::String(grant.step_id.clone()));
    payload.insert(
        "searchStepDigest".into(),
        Value::String(grant.search_step_digest.clone()),
    );
    match kind {
        "bridge_plan.attempt_ack" => {
            payload.insert("status".into(), Value::String("accepted".into()));
        }
        "bridge_plan.step_progress" => {
            payload.insert("status".into(), Value::String("running".into()));
        }
        "bridge_plan.step_result" => {
            payload.insert("status".into(), Value::String("completed".into()));
            payload.insert("safeResult".into(), serde_json::json!({"summary": safe_summary.ok_or_else(|| AppError::InvalidInput("Bridge Plan safe result is required.".into()))?, "candidates": []}));
        }
        "bridge_plan.step_failed" => {
            payload.insert("status".into(), Value::String("failed".into()));
            payload.insert(
                "failureCode".into(),
                Value::String(
                    failure_code
                        .ok_or_else(|| {
                            AppError::InvalidInput("Bridge Plan failure code is required.".into())
                        })?
                        .into(),
                ),
            );
        }
        _ => return invalid("Unsupported Bridge Plan update."),
    }
    Ok(Value::Object(payload))
}

pub(crate) fn transfer_update_payload(
    grant: &TransferExecutionGrant,
    kind: &str,
    safe_summary: Option<&str>,
    failure_code: Option<&str>,
) -> AppResult<Value> {
    let search_grant = SearchExecutionGrant {
        bridge_id: grant.bridge_id.clone(),
        plan_id: grant.plan_id.clone(),
        revision_id: grant.revision_id.clone(),
        revision_hash: grant.revision_hash.clone(),
        requester_device_ref: grant.requester_device_ref.clone(),
        receiver_device_ref: grant.receiver_device_ref.clone(),
        approval_id: grant.approval_id.clone(),
        attempt_id: grant.attempt_id.clone(),
        step_id: grant.step_id.clone(),
        search_step_digest: grant.step_digest.clone(),
        filename_hint: String::new(),
        extensions: Vec::new(),
        safe_scope_labels: Vec::new(),
    };
    attempt_update_payload(&search_grant, kind, safe_summary, failure_code)
}

pub(crate) fn transform_update_payload(
    grant: &TransformExecutionGrant,
    kind: &str,
    safe_summary: Option<&str>,
    failure_code: Option<&str>,
) -> AppResult<Value> {
    transfer_update_payload(
        &TransferExecutionGrant {
            bridge_id: grant.bridge_id.clone(),
            plan_id: grant.plan_id.clone(),
            revision_id: grant.revision_id.clone(),
            revision_hash: grant.revision_hash.clone(),
            requester_device_ref: grant.requester_device_ref.clone(),
            receiver_device_ref: grant.receiver_device_ref.clone(),
            approval_id: grant.approval_id.clone(),
            attempt_id: grant.attempt_id.clone(),
            step_id: grant.step_id.clone(),
            step_digest: grant.step_digest.clone(),
            candidate_id: grant.candidate_id.clone(),
            generated_file: None,
            destination: super::TransferDestination::RequestingDevice {
                device_ref: grant.requester_device_ref.clone(),
            },
        },
        kind,
        safe_summary,
        failure_code,
    )
}

pub(crate) fn attempt_search_result_payload(
    grant: &SearchExecutionGrant,
    result: &BridgePlanSearchResult,
) -> AppResult<Value> {
    if result.status != "completed" {
        return invalid("Bridge Plan Search result is not completed.");
    }
    let summary = format!(
        "Search finished with {} matching file result(s).",
        result.candidates.len()
    );
    let candidates = result
        .candidates
        .iter()
        .map(|candidate| SafeSearchCandidate {
            candidate_id: candidate.candidate_id.clone(),
            display_name: candidate.display_name.clone(),
            redacted_location: candidate.redacted_location.clone(),
            extension: candidate.extension.clone(),
            mime_family: candidate.mime_family.clone(),
            size_bytes: candidate.size_bytes,
            modified_at: candidate.modified_at.clone(),
            match_reason: candidate.match_reason.clone(),
            confidence: candidate.confidence.clone(),
        })
        .collect::<Vec<_>>();
    safe_candidates_value(&candidates)?;
    let mut payload =
        attempt_update_payload(grant, "bridge_plan.step_result", Some(&summary), None)?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan result payload.".into()))?;
    object.insert(
        "safeResult".into(),
        serde_json::json!({
            "summary": summary,
            "candidates": candidates.iter().map(|candidate| serde_json::json!({
                "candidateId": candidate.candidate_id,
                "displayName": candidate.display_name,
                "redactedLocation": candidate.redacted_location,
                "extension": candidate.extension,
                "mimeFamily": candidate.mime_family,
                "sizeBytes": candidate.size_bytes,
                "modifiedAt": candidate.modified_at,
                "matchReason": candidate.match_reason,
                "confidence": candidate.confidence,
            })).collect::<Vec<_>>(),
        }),
    );
    Ok(payload)
}

pub(crate) fn init_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
      CREATE TABLE IF NOT EXISTS bridge_plan_protocol_reviews (
        bridge_id TEXT NOT NULL, direction TEXT NOT NULL, approval_id TEXT NOT NULL,
        plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
        requester_device_ref TEXT NOT NULL, receiver_device_ref TEXT NOT NULL,
        correlation_id TEXT NOT NULL, request_nonce TEXT NOT NULL, search_step_digest TEXT NOT NULL,
        review_expires_at INTEGER NOT NULL, revision_json TEXT NOT NULL,
        decision TEXT, reviewed_at INTEGER, attestation_digest TEXT,
        PRIMARY KEY (bridge_id, direction, request_nonce),
        UNIQUE (bridge_id, direction, correlation_id)
      );
      CREATE TABLE IF NOT EXISTS bridge_plan_protocol_attempts (
        bridge_id TEXT NOT NULL, attempt_id TEXT NOT NULL, approval_id TEXT NOT NULL,
        plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
        requester_device_ref TEXT NOT NULL, receiver_device_ref TEXT NOT NULL,
        step_id TEXT NOT NULL, search_step_digest TEXT NOT NULL, attempt_nonce TEXT NOT NULL,
        expires_at INTEGER NOT NULL, state TEXT NOT NULL, terminal_summary TEXT,
        PRIMARY KEY (bridge_id, attempt_id), UNIQUE (bridge_id, attempt_nonce)
      );
      CREATE TABLE IF NOT EXISTS bridge_plan_protocol_transfer_attempts (
        bridge_id TEXT NOT NULL, attempt_id TEXT NOT NULL, approval_id TEXT NOT NULL,
        plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
        requester_device_ref TEXT NOT NULL, receiver_device_ref TEXT NOT NULL,
        step_id TEXT NOT NULL, step_digest TEXT NOT NULL, attempt_nonce TEXT NOT NULL,
        expires_at INTEGER NOT NULL, state TEXT NOT NULL, terminal_summary TEXT,
        PRIMARY KEY (bridge_id, attempt_id), UNIQUE (bridge_id, attempt_nonce)
      );
      CREATE TABLE IF NOT EXISTS bridge_plan_protocol_transform_attempts (
        bridge_id TEXT NOT NULL, attempt_id TEXT NOT NULL, approval_id TEXT NOT NULL,
        plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision_hash TEXT NOT NULL,
        requester_device_ref TEXT NOT NULL, receiver_device_ref TEXT NOT NULL,
        step_id TEXT NOT NULL, step_digest TEXT NOT NULL, attempt_nonce TEXT NOT NULL,
        expires_at INTEGER NOT NULL, state TEXT NOT NULL, terminal_summary TEXT,
        PRIMARY KEY (bridge_id, attempt_id), UNIQUE (bridge_id, attempt_nonce)
      );
    "#,
    )?;
    Ok(())
}

pub(crate) fn delete_bridge_records(tx: &Transaction<'_>, bridge_id: &str) -> AppResult<()> {
    tx.execute(
        "DELETE FROM bridge_plan_protocol_attempts WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_protocol_transfer_attempts WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_protocol_transform_attempts WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    tx.execute(
        "DELETE FROM bridge_plan_protocol_reviews WHERE bridge_id = ?1",
        [bridge_id],
    )?;
    Ok(())
}

pub(crate) fn reconcile_protocol_startup(paths: &AppPaths, _now: i64) -> AppResult<usize> {
    let conn = connection(paths)?;
    let search = conn.execute("UPDATE bridge_plan_protocol_attempts SET state = 'interrupted', terminal_summary = 'application_restarted' WHERE state IN ('accepted','running')", [])?;
    let transfer = conn.execute("UPDATE bridge_plan_protocol_transfer_attempts SET state = 'interrupted', terminal_summary = 'application_restarted' WHERE state IN ('accepted','running')", [])?;
    let transform = conn.execute("UPDATE bridge_plan_protocol_transform_attempts SET state = 'interrupted', terminal_summary = 'application_restarted' WHERE state IN ('accepted','running')", [])?;
    Ok(search + transfer + transform)
}

#[derive(Clone)]
struct Common {
    bridge: String,
    plan: String,
    revision: String,
    hash: String,
    requester: String,
    receiver: String,
}
#[derive(Clone)]
struct Review {
    common: Common,
    approval: String,
    correlation: String,
    nonce: String,
    expires: i64,
    digest: String,
    revision: BridgePlanRevision,
}
#[derive(Clone)]
struct Decision {
    common: Common,
    approval: String,
    correlation: String,
    nonce: String,
    expires: i64,
    digest: String,
    decision: ReceiverDecision,
    reviewed: i64,
    attestation: String,
}
#[derive(Clone)]
struct Attempt {
    common: Common,
    approval: String,
    id: String,
    step: String,
    digest: String,
    nonce: String,
    expires: i64,
    state: String,
    summary: Option<String>,
}
#[derive(Clone)]
struct Selection {
    common: Common,
    approval: String,
    id: String,
    step: String,
    digest: String,
    candidate: String,
}

pub(crate) fn protocol_metadata(
    kind: &str,
    payload: &Map<String, Value>,
    bridge: &str,
    sender: &str,
    receiver: &str,
    now: i64,
) -> AppResult<ProtocolMetadata> {
    let common = common_for_event(payload, kind, bridge, sender, receiver)?;
    let replay_id = match kind {
        "bridge_plan.review_request" => format!(
            "bridge-plan-review:{}",
            review(payload, &common, now)?.nonce
        ),
        "bridge_plan.review_decision" => format!(
            "bridge-plan-decision:{}",
            decision(payload, &common, now)?.nonce
        ),
        "bridge_plan.attempt_start" => format!(
            "bridge-plan-attempt:{}",
            start(payload, &common, now)?.nonce
        ),
        "bridge_plan.transfer_start" => format!(
            "bridge-plan-transfer:{}",
            transfer_start(payload, &common, now)?.nonce
        ),
        "bridge_plan.transform_start" => format!(
            "bridge-plan-transform:{}",
            transform_start(payload, &common, now)?.nonce
        ),
        "bridge_plan.search_selection" => {
            let selection = selection(payload, &common, now)?;
            format!(
                "bridge-plan-selection:{}:{}",
                selection.id, selection.candidate
            )
        }
        "bridge_plan.attempt_ack"
        | "bridge_plan.step_progress"
        | "bridge_plan.step_result"
        | "bridge_plan.step_failed"
        | "bridge_plan.cancel" => {
            let update = update(payload, &common, kind)?;
            format!("bridge-plan-update:{}:{}:{}", update.id, update.step, kind)
        }
        _ => return invalid("Unsupported Bridge Plan protocol event kind."),
    };
    Ok(ProtocolMetadata { replay_id })
}

/// Sender-side persistence is intentionally limited to review correlation; it
/// does not create an attempt or execution authority.
pub(crate) fn record_outbound_protocol_event(
    paths: &AppPaths,
    kind: &str,
    event: &Value,
    now: i64,
) -> AppResult<()> {
    let payload = payload(event)?;
    let bridge = string(payload, "bridgeId", 128)?;
    match kind {
        "bridge_plan.review_request" => {
            let sender = string(payload, "requesterDeviceRef", 128)?;
            let receiver = string(payload, "receiverDeviceRef", 128)?;
            let review = review(payload, &common(payload, &bridge, &sender, &receiver)?, now)?;
            insert_review(paths, "requester", &review)
        }
        "bridge_plan.review_decision" => {
            let requester = string(payload, "requesterDeviceRef", 128)?;
            let receiver = string(payload, "receiverDeviceRef", 128)?;
            let decision = decision(
                payload,
                &common(payload, &bridge, &requester, &receiver)?,
                now,
            )?;
            record_receiver_decision(paths, &decision)
        }
        "bridge_plan.attempt_start" => {
            let requester = string(payload, "requesterDeviceRef", 128)?;
            let receiver = string(payload, "receiverDeviceRef", 128)?;
            let attempt = start(
                payload,
                &common(payload, &bridge, &requester, &receiver)?,
                now,
            )?;
            insert_attempt(paths, &attempt)
        }
        "bridge_plan.transfer_start" => {
            let requester = string(payload, "requesterDeviceRef", 128)?;
            let receiver = string(payload, "receiverDeviceRef", 128)?;
            let attempt = transfer_start(
                payload,
                &common(payload, &bridge, &requester, &receiver)?,
                now,
            )?;
            insert_transfer_attempt(paths, &attempt)
        }
        "bridge_plan.transform_start" => {
            let requester = string(payload, "requesterDeviceRef", 128)?;
            let receiver = string(payload, "receiverDeviceRef", 128)?;
            let attempt = transform_start(
                payload,
                &common(payload, &bridge, &requester, &receiver)?,
                now,
            )?;
            insert_transform_attempt(paths, &attempt)
        }
        _ => Ok(()),
    }
}

/// The receiver must retain its exact reviewed decision before it sends the
/// attestation. Attempt-start checks this receiver-local record, so a restart
/// never turns an unrecorded click into execution authority.
fn record_receiver_decision(paths: &AppPaths, decision: &Decision) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let stored = tx.query_row(
        "SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,correlation_id,request_nonce,search_step_digest,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND request_nonce=?2",
        params![decision.common.bridge, decision.nonce],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,String>(7)?,row.get::<_,String>(8)?,row.get::<_,i64>(9)?,row.get::<_,Option<String>>(10)?)),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Bridge Plan receiver review not found.".into()))?;
    if stored.0 != decision.approval
        || stored.1 != decision.common.plan
        || stored.2 != decision.common.revision
        || stored.3 != decision.common.hash
        || stored.4 != decision.common.requester
        || stored.5 != decision.common.receiver
        || stored.6 != decision.correlation
        || stored.7 != decision.nonce
        || stored.8 != decision.digest
        || stored.9 != decision.expires
        || stored.10.is_some()
    {
        return invalid("Bridge Plan receiver decision does not bind review.");
    }
    if tx.execute(
        "UPDATE bridge_plan_protocol_reviews SET decision=?1,reviewed_at=?2,attestation_digest=?3 WHERE bridge_id=?4 AND direction='receiver' AND request_nonce=?5 AND decision IS NULL",
        params![if decision.decision == ReceiverDecision::Approved { "allow" } else { "deny" }, decision.reviewed, decision.attestation, decision.common.bridge, decision.nonce],
    )? != 1 {
        return invalid("Bridge Plan receiver decision became stale.");
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn accept_inbound_protocol_event(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    candidates: &mut crate::file_candidates::BridgePlanCandidateStore,
    kind: &str,
    event: &Value,
    now: i64,
) -> AppResult<()> {
    if !kind.starts_with("bridge_plan.") {
        return Ok(());
    }
    let payload = payload(event)?;
    let bridge = string(payload, "bridgeId", 128)?;
    let outer = event
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol event.".into()))?;
    let source = outer
        .get("sourceDeviceRef")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol event.".into()))?;
    let target = outer
        .get("targetPeerRef")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol event.".into()))?;
    let common = common_for_event(payload, kind, &bridge, source, target)?;
    match kind {
        "bridge_plan.review_request" => {
            insert_review(paths, "receiver", &review(payload, &common, now)?)?
        }
        "bridge_plan.review_decision" => accept_decision(paths, payload, &common, now)?,
        "bridge_plan.attempt_start" => accept_start(paths, authorities, payload, &common, now)?,
        "bridge_plan.transfer_start" => {
            accept_transfer_start(paths, authorities, payload, &common, now)?
        }
        "bridge_plan.transform_start" => {
            accept_transform_start(paths, authorities, payload, &common, now)?
        }
        "bridge_plan.search_selection" => {
            accept_selection(paths, authorities, candidates, payload, &common, now)?
        }
        "bridge_plan.attempt_ack"
        | "bridge_plan.step_progress"
        | "bridge_plan.step_result"
        | "bridge_plan.step_failed"
        | "bridge_plan.cancel" => accept_update(paths, authorities, payload, &common, kind)?,
        _ => return invalid("Unsupported Bridge Plan protocol event kind."),
    }
    Ok(())
}

fn common(
    value: &Map<String, Value>,
    bridge: &str,
    requester: &str,
    receiver: &str,
) -> AppResult<Common> {
    required(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
        ],
    )?;
    if string(value, "schemaVersion", 128)? != PROTOCOL_VERSION
        || string(value, "bridgeId", 128)? != bridge
        || string(value, "requesterDeviceRef", 128)? != requester
        || string(value, "receiverDeviceRef", 128)? != receiver
    {
        return invalid("Bridge Plan protocol identity mismatch.");
    }
    Ok(Common {
        bridge: bridge.into(),
        plan: string(value, "planId", 128)?,
        revision: string(value, "revisionId", 128)?,
        hash: string(value, "revisionHash", 256)?,
        requester: requester.into(),
        receiver: receiver.into(),
    })
}

fn common_for_event(
    value: &Map<String, Value>,
    kind: &str,
    bridge: &str,
    source: &str,
    target: &str,
) -> AppResult<Common> {
    let requester = string(value, "requesterDeviceRef", 128)?;
    let receiver = string(value, "receiverDeviceRef", 128)?;
    let requester_sends = matches!(
        kind,
        "bridge_plan.review_request"
            | "bridge_plan.attempt_start"
            | "bridge_plan.transfer_start"
            | "bridge_plan.transform_start"
            | "bridge_plan.search_selection"
            | "bridge_plan.cancel"
    );
    if (requester_sends && (requester != source || receiver != target))
        || (!requester_sends && (receiver != source || requester != target))
    {
        return invalid("Bridge Plan protocol sender or receiver mismatch.");
    }
    common(value, bridge, &requester, &receiver)
}

fn review(value: &Map<String, Value>, common: &Common, now: i64) -> AppResult<Review> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "correlationId",
            "requestNonce",
            "reviewExpiresAt",
            "revision",
            "searchStep",
            "searchStepDigest",
        ],
        &[],
    )?;
    let raw = value
        .get("revision")
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan review revision.".into()))?;
    let revision: BridgePlanRevision = serde_json::from_value(raw.clone())
        .map_err(|_| AppError::InvalidInput("Invalid Bridge Plan review revision.".into()))?;
    if serde_json::to_value(&revision)? != raw
        || canonical_revision_hash(&revision)? != common.hash
        || revision.plan_id != common.plan
        || revision.revision_id != common.revision
        || revision.bridge_id != common.bridge
        || revision.requesting_device_ref != common.requester
        || revision.selected_device_ref != common.receiver
    {
        return invalid("Bridge Plan review revision mismatch.");
    }
    let raw_step = value
        .get("searchStep")
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan Search step.".into()))?;
    let step: BridgePlanStep = serde_json::from_value(raw_step.clone())
        .map_err(|_| AppError::InvalidInput("Invalid Bridge Plan Search step.".into()))?;
    if serde_json::to_value(&step)? != raw_step
        || !matches!(step, BridgePlanStep::Search { .. })
        || !revision.steps.iter().any(|candidate| candidate == &step)
    {
        return invalid("Bridge Plan Search step mismatch.");
    }
    let digest = step_digest(&step)?;
    if string(value, "searchStepDigest", 256)? != digest {
        return invalid("Bridge Plan Search digest mismatch.");
    }
    let expires = integer(value, "reviewExpiresAt")?;
    if expires <= now || expires > now + MAX_LIFETIME {
        return invalid("Bridge Plan review expiry is invalid.");
    }
    Ok(Review {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        correlation: string(value, "correlationId", 128)?,
        nonce: string(value, "requestNonce", 128)?,
        expires,
        digest,
        revision,
    })
}

fn decision(value: &Map<String, Value>, common: &Common, now: i64) -> AppResult<Decision> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "correlationId",
            "requestNonce",
            "reviewExpiresAt",
            "searchStepDigest",
            "decision",
            "reviewedAt",
            "attestationVersion",
            "attestationDigest",
        ],
        &[],
    )?;
    let decision = match string(value, "decision", 16)?.as_str() {
        "allow" => ReceiverDecision::Approved,
        "deny" => ReceiverDecision::Denied,
        _ => return invalid("Invalid Bridge Plan decision."),
    };
    let expires = integer(value, "reviewExpiresAt")?;
    let reviewed = integer(value, "reviewedAt")?;
    if expires <= now
        || expires > now + MAX_LIFETIME
        || reviewed <= 0
        || string(value, "attestationVersion", 128)? != PROTOCOL_VERSION
    {
        return invalid("Invalid Bridge Plan attestation timing.");
    }
    let attestation = attestation_digest(value)?;
    if string(value, "attestationDigest", 256)? != attestation {
        return invalid("Bridge Plan attestation digest mismatch.");
    }
    Ok(Decision {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        correlation: string(value, "correlationId", 128)?,
        nonce: string(value, "requestNonce", 128)?,
        expires,
        digest: string(value, "searchStepDigest", 256)?,
        decision,
        reviewed,
        attestation,
    })
}

fn start(value: &Map<String, Value>, common: &Common, now: i64) -> AppResult<Attempt> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "attemptId",
            "searchStep",
            "searchStepDigest",
            "attemptNonce",
            "attemptExpiresAt",
        ],
        &[],
    )?;
    let raw = value
        .get("searchStep")
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan attempt step.".into()))?;
    let step: BridgePlanStep = serde_json::from_value(raw.clone())
        .map_err(|_| AppError::InvalidInput("Invalid Bridge Plan attempt step.".into()))?;
    if serde_json::to_value(&step)? != raw || !matches!(step, BridgePlanStep::Search { .. }) {
        return invalid("Bridge Plan attempt must be exact Search.");
    }
    let digest = step_digest(&step)?;
    if string(value, "searchStepDigest", 256)? != digest {
        return invalid("Bridge Plan attempt digest mismatch.");
    }
    let expires = integer(value, "attemptExpiresAt")?;
    if expires <= now || expires > now + MAX_LIFETIME {
        return invalid("Bridge Plan attempt expiry is invalid.");
    }
    Ok(Attempt {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        id: string(value, "attemptId", 128)?,
        step: search_step_id(&step)?,
        digest,
        nonce: string(value, "attemptNonce", 128)?,
        expires,
        state: "accepted".into(),
        summary: None,
    })
}

fn transfer_start(value: &Map<String, Value>, common: &Common, now: i64) -> AppResult<Attempt> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "attemptId",
            "transferStep",
            "transferStepDigest",
            "attemptNonce",
            "attemptExpiresAt",
        ],
        &[],
    )?;
    let raw = value
        .get("transferStep")
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan Transfer step.".into()))?;
    let step: BridgePlanStep = serde_json::from_value(raw.clone())
        .map_err(|_| AppError::InvalidInput("Invalid Bridge Plan Transfer step.".into()))?;
    if serde_json::to_value(&step)? != raw || !matches!(step, BridgePlanStep::Transfer { .. }) {
        return invalid("Bridge Plan attempt must be exact Transfer.");
    }
    let digest = step_digest(&step)?;
    if string(value, "transferStepDigest", 256)? != digest {
        return invalid("Bridge Plan Transfer digest mismatch.");
    }
    let expires = integer(value, "attemptExpiresAt")?;
    if expires <= now || expires > now + MAX_LIFETIME {
        return invalid("Bridge Plan Transfer expiry is invalid.");
    }
    Ok(Attempt {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        id: string(value, "attemptId", 128)?,
        step: transfer_step_id(&step)?,
        digest,
        nonce: string(value, "attemptNonce", 128)?,
        expires,
        state: "accepted".into(),
        summary: None,
    })
}

fn transform_start(value: &Map<String, Value>, common: &Common, now: i64) -> AppResult<Attempt> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "attemptId",
            "transformStep",
            "transformStepDigest",
            "attemptNonce",
            "attemptExpiresAt",
        ],
        &[],
    )?;
    let raw = value
        .get("transformStep")
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan Transform step.".into()))?;
    let step: BridgePlanStep = serde_json::from_value(raw.clone())
        .map_err(|_| AppError::InvalidInput("Invalid Bridge Plan Transform step.".into()))?;
    if serde_json::to_value(&step)? != raw || !matches!(step, BridgePlanStep::Transform { .. }) {
        return invalid("Bridge Plan attempt must be exact Transform.");
    }
    let digest = step_digest(&step)?;
    if string(value, "transformStepDigest", 256)? != digest {
        return invalid("Bridge Plan Transform digest mismatch.");
    }
    let expires = integer(value, "attemptExpiresAt")?;
    if expires <= now || expires > now + MAX_LIFETIME {
        return invalid("Bridge Plan Transform expiry is invalid.");
    }
    Ok(Attempt {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        id: string(value, "attemptId", 128)?,
        step: transfer_step_id(&step)?,
        digest,
        nonce: string(value, "attemptNonce", 128)?,
        expires,
        state: "accepted".into(),
        summary: None,
    })
}

fn selection(value: &Map<String, Value>, common: &Common, _now: i64) -> AppResult<Selection> {
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
            "approvalId",
            "attemptId",
            "stepId",
            "searchStepDigest",
            "candidateId",
        ],
        &[],
    )?;
    let candidate = string(value, "candidateId", 128)?;
    if candidate.starts_with('/') || candidate.contains('/') || candidate.contains('\\') {
        return invalid("Bridge Plan candidate ID is not opaque.");
    }
    Ok(Selection {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        id: string(value, "attemptId", 128)?,
        step: string(value, "stepId", 128)?,
        digest: string(value, "searchStepDigest", 256)?,
        candidate,
    })
}

fn update(value: &Map<String, Value>, common: &Common, kind: &str) -> AppResult<Attempt> {
    let (required, status) = match kind {
        "bridge_plan.attempt_ack" => (
            &[
                "approvalId",
                "attemptId",
                "stepId",
                "searchStepDigest",
                "status",
            ][..],
            "accepted",
        ),
        "bridge_plan.step_progress" => (
            &[
                "approvalId",
                "attemptId",
                "stepId",
                "searchStepDigest",
                "status",
            ][..],
            "running",
        ),
        "bridge_plan.step_result" => (
            &[
                "approvalId",
                "attemptId",
                "stepId",
                "searchStepDigest",
                "status",
                "safeResult",
            ][..],
            "completed",
        ),
        "bridge_plan.step_failed" => (
            &[
                "approvalId",
                "attemptId",
                "stepId",
                "searchStepDigest",
                "status",
                "failureCode",
            ][..],
            "failed",
        ),
        "bridge_plan.cancel" => (
            &[
                "approvalId",
                "attemptId",
                "stepId",
                "searchStepDigest",
                "status",
            ][..],
            "cancelled",
        ),
        _ => return invalid("Unsupported Bridge Plan protocol event kind."),
    };
    exact(
        value,
        &[
            "schemaVersion",
            "bridgeId",
            "planId",
            "revisionId",
            "revisionHash",
            "requesterDeviceRef",
            "receiverDeviceRef",
        ],
        required,
    )?;
    if string(value, "status", 32)? != status {
        return invalid("Bridge Plan update status mismatch.");
    }
    let summary = if kind == "bridge_plan.step_result" {
        let safe = value
            .get("safeResult")
            .and_then(Value::as_object)
            .ok_or_else(|| AppError::InvalidInput("Invalid safe result.".into()))?;
        exact(safe, &["summary", "candidates"], &[])?;
        let candidates = safe
            .get("candidates")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::InvalidInput("Invalid safe Search candidates.".into()))?;
        if candidates.len() > 10 {
            return invalid("Bridge Plan Search result exceeds its candidate bound.");
        }
        for candidate in candidates {
            validate_safe_candidate(candidate)?;
        }
        Some(string(safe, "summary", 512)?)
    } else {
        None
    };
    if kind == "bridge_plan.step_failed" {
        let _ = string(value, "failureCode", 64)?;
    }
    Ok(Attempt {
        common: common.clone(),
        approval: string(value, "approvalId", 128)?,
        id: string(value, "attemptId", 128)?,
        step: string(value, "stepId", 128)?,
        digest: string(value, "searchStepDigest", 256)?,
        nonce: String::new(),
        expires: 0,
        state: status.into(),
        summary,
    })
}

fn insert_review(paths: &AppPaths, direction: &str, review: &Review) -> AppResult<()> {
    let conn = connection(paths)?;
    if conn.execute("INSERT OR IGNORE INTO bridge_plan_protocol_reviews (bridge_id,direction,approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,correlation_id,request_nonce,search_step_digest,review_expires_at,revision_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![review.common.bridge,direction,review.approval,review.common.plan,review.common.revision,review.common.hash,review.common.requester,review.common.receiver,review.correlation,review.nonce,review.digest,review.expires,json(&review.revision)?])? != 1 { return invalid("Bridge Plan review replayed."); }
    Ok(())
}

fn accept_decision(
    paths: &AppPaths,
    value: &Map<String, Value>,
    common: &Common,
    now: i64,
) -> AppResult<()> {
    let decision = decision(value, common, now)?;
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let stored=tx.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,correlation_id,request_nonce,search_step_digest,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='requester' AND request_nonce=?2",params![common.bridge,decision.nonce],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,i64>(9)?,r.get::<_,Option<String>>(10)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan review not found.".into()))?;
    if stored.0 != decision.approval
        || stored.1 != common.plan
        || stored.2 != common.revision
        || stored.3 != common.hash
        || stored.4 != common.requester
        || stored.5 != common.receiver
        || stored.6 != decision.correlation
        || stored.7 != decision.nonce
        || stored.8 != decision.digest
        || stored.9 != decision.expires
        || stored.10.is_some()
    {
        return invalid("Bridge Plan decision does not bind review.");
    }
    tx.execute("UPDATE bridge_plan_protocol_reviews SET decision=?1,reviewed_at=?2,attestation_digest=?3 WHERE bridge_id=?4 AND direction='requester' AND request_nonce=?5 AND decision IS NULL",params![if decision.decision==ReceiverDecision::Approved{"allow"}else{"deny"},decision.reviewed,decision.attestation,common.bridge,decision.nonce])?;
    tx.commit()?;
    BridgePlanStore::new(paths).decide_receiver(
        &decision.approval,
        &ReceiverDecisionEvidence {
            revision_hash: common.hash.clone(),
            receiver_device_ref: common.receiver.clone(),
            decision: decision.decision,
            reviewed_at: decision.reviewed,
            evidence_digest: decision.attestation,
        },
    )
}

fn accept_start(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    value: &Map<String, Value>,
    common: &Common,
    now: i64,
) -> AppResult<()> {
    let attempt = start(value, common, now)?;
    let conn = connection(paths)?;
    let review=conn.query_row("SELECT plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,search_step_digest,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",params![common.bridge,attempt.approval],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,i64>(6)?,r.get::<_,Option<String>>(7)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan receiver review missing.".into()))?;
    if review.0 != common.plan
        || review.1 != common.revision
        || review.2 != common.hash
        || review.3 != common.requester
        || review.4 != common.receiver
        || review.5 != attempt.digest
        || review.6 <= now
        || review.7.as_deref() != Some("allow")
    {
        return invalid("Bridge Plan attempt is not approved by receiver.");
    }
    drop(conn);
    insert_attempt(paths, &attempt)?;
    authorities.grant(&common.bridge, &attempt.id, &attempt.step, attempt.expires)
}

fn insert_attempt(paths: &AppPaths, attempt: &Attempt) -> AppResult<()> {
    let conn = connection(paths)?;
    if conn.execute("INSERT OR IGNORE INTO bridge_plan_protocol_attempts (bridge_id,attempt_id,approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,search_step_digest,attempt_nonce,expires_at,state) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'accepted')",params![attempt.common.bridge,attempt.id,attempt.approval,attempt.common.plan,attempt.common.revision,attempt.common.hash,attempt.common.requester,attempt.common.receiver,attempt.step,attempt.digest,attempt.nonce,attempt.expires])? !=1{return invalid("Bridge Plan attempt replayed.");}
    Ok(())
}

fn insert_transfer_attempt(paths: &AppPaths, attempt: &Attempt) -> AppResult<()> {
    let conn = connection(paths)?;
    if conn.execute("INSERT OR IGNORE INTO bridge_plan_protocol_transfer_attempts (bridge_id,attempt_id,approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest,attempt_nonce,expires_at,state) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'accepted')",params![attempt.common.bridge,attempt.id,attempt.approval,attempt.common.plan,attempt.common.revision,attempt.common.hash,attempt.common.requester,attempt.common.receiver,attempt.step,attempt.digest,attempt.nonce,attempt.expires])? !=1{return invalid("Bridge Plan Transfer replayed.");}
    Ok(())
}

fn insert_transform_attempt(paths: &AppPaths, attempt: &Attempt) -> AppResult<()> {
    let conn = connection(paths)?;
    if conn.execute("INSERT OR IGNORE INTO bridge_plan_protocol_transform_attempts (bridge_id,attempt_id,approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest,attempt_nonce,expires_at,state) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'accepted')",params![attempt.common.bridge,attempt.id,attempt.approval,attempt.common.plan,attempt.common.revision,attempt.common.hash,attempt.common.requester,attempt.common.receiver,attempt.step,attempt.digest,attempt.nonce,attempt.expires])? !=1{return invalid("Bridge Plan Transform replayed.");}
    Ok(())
}

fn accept_transfer_start(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    value: &Map<String, Value>,
    common: &Common,
    now: i64,
) -> AppResult<()> {
    let attempt = transfer_start(value, common, now)?;
    let conn = connection(paths)?;
    let review=conn.query_row("SELECT revision_json,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",params![common.bridge,attempt.approval],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan receiver review missing.".into()))?;
    if review.1 <= now || review.2.as_deref() != Some("allow") {
        return invalid("Bridge Plan Transfer is not approved by receiver.");
    }
    let revision: BridgePlanRevision = serde_json::from_str(&review.0)?;
    if revision.plan_id != common.plan
        || revision.revision_id != common.revision
        || revision.revision_hash != common.hash
        || revision.requesting_device_ref != common.requester
        || revision.selected_device_ref != common.receiver
    {
        return invalid("Bridge Plan Transfer review binding mismatch.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == attempt.step)
        .ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transfer step is unavailable.".into())
        })?;
    if !matches!(step, BridgePlanStep::Transfer { .. }) || step_digest(step)? != attempt.digest {
        return invalid("Bridge Plan Transfer step mismatch.");
    }
    drop(conn);
    let BridgePlanStep::Transfer { source, .. } = step else {
        unreachable!()
    };
    match source {
        super::ObjectSelectionRule::FromSlot { slot_id } if slot_id == "selected_file" => {
            authorities.selected_candidate_id(&common.bridge, &attempt.id, now)?;
        }
        super::ObjectSelectionRule::FromSlot { slot_id } if slot_id == "transformed_file" => {
            // The output remains Rust-private; its presence proves the prior
            // approved Transform completed in this receiver process.
            if !authorities.has_transform_output(&common.bridge, &attempt.id)? {
                return invalid(
                    "Bridge Plan Transform result is unavailable after restart or expiry.",
                );
            }
        }
        super::ObjectSelectionRule::FutureUserSelection { .. } => {}
        _ => return invalid("Bridge Plan Transfer source is not available yet."),
    }
    insert_transfer_attempt(paths, &attempt)?;
    authorities.grant_transfer(&common.bridge, &attempt.id, &attempt.step, attempt.expires)
}

fn accept_transform_start(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    value: &Map<String, Value>,
    common: &Common,
    now: i64,
) -> AppResult<()> {
    let attempt = transform_start(value, common, now)?;
    let conn = connection(paths)?;
    let review=conn.query_row("SELECT revision_json,review_expires_at,decision FROM bridge_plan_protocol_reviews WHERE bridge_id=?1 AND direction='receiver' AND approval_id=?2",params![common.bridge,attempt.approval],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan receiver review missing.".into()))?;
    if review.1 <= now || review.2.as_deref() != Some("allow") {
        return invalid("Bridge Plan Transform is not approved by receiver.");
    }
    let revision: BridgePlanRevision = serde_json::from_str(&review.0)?;
    if revision.plan_id != common.plan
        || revision.revision_id != common.revision
        || revision.revision_hash != common.hash
        || revision.requesting_device_ref != common.requester
        || revision.selected_device_ref != common.receiver
    {
        return invalid("Bridge Plan Transform review binding mismatch.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == attempt.step)
        .ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Transform step is unavailable.".into())
        })?;
    if !matches!(step, BridgePlanStep::Transform { .. }) || step_digest(step)? != attempt.digest {
        return invalid("Bridge Plan Transform step mismatch.");
    }
    drop(conn);
    authorities.selected_candidate_id(&common.bridge, &attempt.id, now)?;
    insert_transform_attempt(paths, &attempt)?;
    authorities.grant_transform(&common.bridge, &attempt.id, &attempt.step, attempt.expires)
}

fn accept_selection(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    candidates: &mut crate::file_candidates::BridgePlanCandidateStore,
    value: &Map<String, Value>,
    common: &Common,
    now: i64,
) -> AppResult<()> {
    let selection = selection(value, common, now)?;
    let conn = connection(paths)?;
    let stored=conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,search_step_digest,expires_at,state FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2",params![common.bridge,selection.id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,i64>(8)?,r.get::<_,String>(9)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan Search attempt is unavailable.".into()))?;
    if stored.0 != selection.approval
        || stored.1 != common.plan
        || stored.2 != common.revision
        || stored.3 != common.hash
        || stored.4 != common.requester
        || stored.5 != common.receiver
        || stored.6 != selection.step
        || stored.7 != selection.digest
        || stored.8 <= now
        || matches!(stored.9.as_str(), "failed" | "cancelled" | "interrupted")
    {
        return invalid("Bridge Plan candidate selection does not bind its attempt.");
    }
    crate::file_candidates::validate_bridge_plan_candidate_selection(
        candidates,
        &common.bridge,
        &common.requester,
        &common.receiver,
        &selection.id,
        &selection.candidate,
    )?;
    authorities.bind_selection(
        &common.bridge,
        &selection.id,
        &selection.candidate,
        stored.8,
    )
}

fn accept_update(
    paths: &AppPaths,
    authorities: &ProtocolSearchAuthorityStore,
    value: &Map<String, Value>,
    common: &Common,
    kind: &str,
) -> AppResult<()> {
    let update = update(value, common, kind)?;
    let conn = connection(paths)?;
    let stored=conn.query_row("SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,search_step_digest,state,'search' FROM bridge_plan_protocol_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND step_id=?3 UNION ALL SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest,state,'transfer' FROM bridge_plan_protocol_transfer_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND step_id=?3 UNION ALL SELECT approval_id,plan_id,revision_id,revision_hash,requester_device_ref,receiver_device_ref,step_id,step_digest,state,'transform' FROM bridge_plan_protocol_transform_attempts WHERE bridge_id=?1 AND attempt_id=?2 AND step_id=?3",params![common.bridge,update.id,update.step],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?))).optional()?.ok_or_else(||AppError::InvalidInput("Bridge Plan attempt missing.".into()))?;
    if stored.0 != update.approval
        || stored.1 != common.plan
        || stored.2 != common.revision
        || stored.3 != common.hash
        || stored.4 != common.requester
        || stored.5 != common.receiver
        || stored.6 != update.step
        || stored.7 != update.digest
    {
        return invalid("Bridge Plan update correlation mismatch.");
    }
    if kind == "bridge_plan.attempt_ack" {
        return Ok(());
    }
    let legal = matches!(
        (stored.8.as_str(), update.state.as_str()),
        ("accepted", "running")
            | ("running", "completed")
            | ("running", "failed")
            | ("accepted", "cancelled")
            | ("running", "cancelled")
    );
    if !legal {
        return invalid("Bridge Plan update is out of order.");
    }
    let table = match stored.9.as_str() {
        "search" => "bridge_plan_protocol_attempts",
        "transfer" => "bridge_plan_protocol_transfer_attempts",
        "transform" => "bridge_plan_protocol_transform_attempts",
        _ => return invalid("Bridge Plan execution kind is invalid."),
    };
    if conn.execute(&format!("UPDATE {table} SET state=?1,terminal_summary=?2 WHERE bridge_id=?3 AND attempt_id=?4 AND state=?5"),params![update.state,update.summary,common.bridge,update.id,stored.8])? != 1 { return invalid("Bridge Plan update became stale."); }
    let store = BridgePlanStore::new(paths);
    let at = crate::storage::now_ts();
    match update.state.as_str() {
        "running" => {
            let attempt = store.list_attempt(&update.id)?;
            if attempt.state == AttemptState::Created {
                store.transition_attempt(&update.id, AttemptState::Running, at)?;
            }
            let current = attempt
                .steps
                .iter()
                .find(|step| step.step_id == update.step)
                .ok_or_else(|| AppError::InvalidInput("Bridge Plan step is unavailable.".into()))?;
            if current.state == StepExecutionState::Eligible {
                store.transition_step(
                    &update.id,
                    &update.step,
                    StepExecutionState::Authorized,
                    at,
                )?;
            }
            store.transition_step(&update.id, &update.step, StepExecutionState::Running, at)?;
            store.append_activity(&BridgePlanActivity {
                activity_id: format!("plan-step-running-{}", uuid::Uuid::new_v4()),
                bridge_id: common.bridge.clone(),
                plan_id: common.plan.clone(),
                revision_id: common.revision.clone(),
                attempt_id: Some(update.id.clone()),
                step_id: Some(update.step.clone()),
                kind: ActivityKind::AttemptStarted,
                occurred_at: at,
                summary: SafeActivitySummary::from(if stored.9 == "transfer" {
                    "Transfer is running on the selected device."
                } else {
                    "Search is running on the selected device."
                }),
            })?;
        }
        "completed" => {
            store.transition_step(&update.id, &update.step, StepExecutionState::Completed, at)?;
            let attempt = store.list_attempt(&update.id)?;
            if attempt
                .steps
                .iter()
                .all(|step| step.state == StepExecutionState::Completed)
            {
                store.transition_attempt(&update.id, AttemptState::Completed, at)?;
            }
            let summary = update.summary.clone().ok_or_else(|| {
                AppError::InvalidInput("Bridge Plan result summary is unavailable.".into())
            })?;
            store.append_result(&BridgePlanResultSummary {
                result_id: format!("plan-result-{}", uuid::Uuid::new_v4()),
                bridge_id: common.bridge.clone(),
                plan_id: common.plan.clone(),
                revision_id: common.revision.clone(),
                attempt_id: update.id.clone(),
                step_id: update.step.clone(),
                status: GeneratedUserVisibleText::from_semantic("completed"),
                summary: SafeActivitySummary::from(summary.clone()),
                produced_object_description: None,
                created_at: at,
            })?;
            store.append_activity(&BridgePlanActivity {
                activity_id: format!("plan-step-completed-{}", uuid::Uuid::new_v4()),
                bridge_id: common.bridge.clone(),
                plan_id: common.plan.clone(),
                revision_id: common.revision.clone(),
                attempt_id: Some(update.id.clone()),
                step_id: Some(update.step.clone()),
                kind: ActivityKind::ResultRecorded,
                occurred_at: at,
                summary: SafeActivitySummary::from(summary),
            })?;
        }
        "failed" => {
            store.transition_step(&update.id, &update.step, StepExecutionState::Failed, at)?;
            store.transition_attempt(&update.id, AttemptState::Failed, at)?;
            store.append_activity(&BridgePlanActivity {
                activity_id: format!("plan-step-failed-{}", uuid::Uuid::new_v4()),
                bridge_id: common.bridge.clone(),
                plan_id: common.plan.clone(),
                revision_id: common.revision.clone(),
                attempt_id: Some(update.id.clone()),
                step_id: Some(update.step.clone()),
                kind: ActivityKind::AttemptFailed,
                occurred_at: at,
                summary: SafeActivitySummary::from(if stored.9 == "transfer" {
                    "Transfer could not be completed on the selected device."
                } else {
                    "Search could not be completed on the selected device."
                }),
            })?;
        }
        "cancelled" => {
            let attempt = store.list_attempt(&update.id)?;
            let step = attempt
                .steps
                .iter()
                .find(|step| step.step_id == update.step)
                .ok_or_else(|| AppError::InvalidInput("Bridge Plan step is unavailable.".into()))?;
            if step.state != StepExecutionState::Cancelled {
                store.transition_step(
                    &update.id,
                    &update.step,
                    StepExecutionState::Cancelled,
                    at,
                )?;
            }
            store.transition_attempt(&update.id, AttemptState::Cancelled, at)?;
        }
        _ => return invalid("Unsupported Bridge Plan update."),
    }
    if matches!(update.state.as_str(), "completed" | "failed" | "cancelled") {
        authorities.revoke(&update.id);
        authorities.revoke_transfer(&update.id);
    }
    Ok(())
}

fn payload(event: &Value) -> AppResult<&Map<String, Value>> {
    event
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol payload.".into()))
}
fn safe_candidates_value(candidates: &[SafeSearchCandidate]) -> AppResult<()> {
    if candidates.len() > 10 {
        return invalid("Bridge Plan Search result exceeds its candidate bound.");
    }
    for candidate in candidates {
        let value = serde_json::json!({
            "candidateId": candidate.candidate_id, "displayName": candidate.display_name,
            "redactedLocation": candidate.redacted_location, "extension": candidate.extension,
            "mimeFamily": candidate.mime_family, "sizeBytes": candidate.size_bytes,
            "modifiedAt": candidate.modified_at, "matchReason": candidate.match_reason,
            "confidence": candidate.confidence,
        });
        validate_safe_candidate(&value)?;
    }
    Ok(())
}
fn validate_safe_candidate(value: &Value) -> AppResult<()> {
    let candidate = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("Invalid safe Search candidate.".into()))?;
    exact(
        candidate,
        &[
            "candidateId",
            "displayName",
            "redactedLocation",
            "extension",
            "mimeFamily",
            "sizeBytes",
            "modifiedAt",
            "matchReason",
            "confidence",
        ],
        &[],
    )?;
    let candidate_id = string(candidate, "candidateId", 128)?;
    if candidate_id.starts_with('/') || candidate_id.contains('\\') || candidate_id.contains('/') {
        return invalid("Bridge Plan candidate ID is not opaque.");
    }
    for field in [
        "displayName",
        "redactedLocation",
        "extension",
        "mimeFamily",
        "modifiedAt",
        "matchReason",
        "confidence",
    ] {
        let _ = string(candidate, field, 256)?;
    }
    let _ = candidate
        .get("sizeBytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::InvalidInput("Invalid safe Search candidate size.".into()))?;
    Ok(())
}
fn exact(value: &Map<String, Value>, base: &[&str], extra: &[&str]) -> AppResult<()> {
    let count = base.len() + extra.len();
    if value.len() != count
        || base
            .iter()
            .chain(extra)
            .any(|key| !value.contains_key(*key))
    {
        return invalid("Invalid Bridge Plan protocol fields.");
    }
    Ok(())
}
fn required(value: &Map<String, Value>, fields: &[&str]) -> AppResult<()> {
    if fields.iter().any(|key| !value.contains_key(*key)) {
        return invalid("Invalid Bridge Plan protocol fields.");
    }
    Ok(())
}
fn string(value: &Map<String, Value>, field: &str, max: usize) -> AppResult<String> {
    let value = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol field.".into()))?;
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return invalid("Invalid Bridge Plan protocol field.");
    }
    Ok(value.into())
}
fn integer(value: &Map<String, Value>, field: &str) -> AppResult<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidInput("Invalid Bridge Plan protocol integer.".into()))
}
fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}
fn step_digest(step: &BridgePlanStep) -> AppResult<String> {
    Ok(format!(
        "sha256:{}",
        blake3::hash(canonical_json(&serde_json::to_value(step)?).as_bytes()).to_hex()
    ))
}
fn search_step_id(step: &BridgePlanStep) -> AppResult<String> {
    match step {
        BridgePlanStep::Search { step_id, .. } => {
            id(step_id, "Bridge Plan Search step")?;
            Ok(step_id.clone())
        }
        _ => invalid("Bridge Plan step is not Search."),
    }
}
fn transfer_step_id(step: &BridgePlanStep) -> AppResult<String> {
    match step {
        BridgePlanStep::Transfer { step_id, .. } => {
            id(step_id, "Bridge Plan Transfer step")?;
            Ok(step_id.clone())
        }
        _ => invalid("Bridge Plan step is not Transfer."),
    }
}
fn attestation_digest(value: &Map<String, Value>) -> AppResult<String> {
    let mut bound = value.clone();
    bound.remove("attestationDigest");
    Ok(format!(
        "bridge-plan-attestation-v1:{}",
        blake3::hash(canonical_json(&Value::Object(bound)).as_bytes()).to_hex()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge_plan::{build_file_search_revision, init_schema, BridgePlanApproval},
        storage::AppPaths,
    };
    use std::fs;

    fn paths() -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "pastey-bridge-plan-protocol-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        AppPaths {
            app_data_dir: root.clone(),
            db_path: root.join("db.sqlite"),
            payloads_dir: root.join("payloads"),
            inbox_dir: root.join("inbox"),
            temp_dir: root.join("temp"),
            logs_dir: root.join("logs"),
            config_path: root.join("config.json"),
        }
    }

    fn outer(kind: &str, payload: Value, source: &str, target: &str) -> Value {
        serde_json::json!({"kind": kind, "sourceDeviceRef": source, "targetPeerRef": target, "payload": payload})
    }

    #[test]
    fn transfer_destination_accepts_only_bound_requester_or_receiver_shared_root() {
        let requester = "requester";
        let receiver = "receiver";
        assert!(supported_transfer_destination(
            &super::super::TransferDestination::RequestingDevice {
                device_ref: requester.into(),
            },
            requester,
            receiver,
        ));
        assert!(supported_transfer_destination(
            &super::super::TransferDestination::UserSelectedLocation {
                device_ref: receiver.into(),
                user_visible_location_scope: super::super::SafeLocationDescription::from(
                    "Pastey Shared",
                ),
            },
            requester,
            receiver,
        ));
        assert!(supported_transfer_destination(
            &super::super::TransferDestination::SelectedDevice {
                device_ref: receiver.into(),
            },
            requester,
            receiver,
        ));
        assert!(!supported_transfer_destination(
            &super::super::TransferDestination::UserSelectedLocation {
                device_ref: receiver.into(),
                user_visible_location_scope: super::super::SafeLocationDescription::from(
                    "Downloads",
                ),
            },
            requester,
            receiver,
        ));
        assert!(!supported_transfer_destination(
            &super::super::TransferDestination::LeaveOnProducingDevice {
                device_ref: receiver.into(),
            },
            requester,
            receiver,
        ));
    }

    #[test]
    fn receiver_records_allow_before_start_and_consumes_one_search_grant() {
        let paths = paths();
        let conn = super::super::connection(&paths).unwrap();
        init_schema(&conn).unwrap();
        let now = crate::storage::now_ts();
        let revision = build_file_search_revision(
            "bridge".into(),
            "requester".into(),
            "receiver".into(),
            "Find the report PDF.".into(),
            "report".into(),
            vec!["pdf".into()],
            vec!["documents".into()],
        )
        .unwrap();
        let approval = BridgePlanApproval {
            approval_id: "approval".into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            requester_device_ref: revision.requesting_device_ref.clone(),
            selected_device_ref: revision.selected_device_ref.clone(),
            receiver_required: true,
            expires_at: now + 600,
        };
        let review = review_request_payload(&approval, &revision).unwrap();
        let authorities = ProtocolSearchAuthorityStore::default();
        let mut candidates = crate::file_candidates::BridgePlanCandidateStore::default();
        accept_inbound_protocol_event(
            &paths,
            &authorities,
            &mut candidates,
            "bridge_plan.review_request",
            &outer(
                "bridge_plan.review_request",
                review.clone(),
                "requester",
                "receiver",
            ),
            now,
        )
        .unwrap();
        let decision = receiver_decision_payload(&paths, "bridge", "approval", true, now).unwrap();
        record_outbound_protocol_event(
            &paths,
            "bridge_plan.review_decision",
            &outer(
                "bridge_plan.review_decision",
                decision,
                "receiver",
                "requester",
            ),
            now,
        )
        .unwrap();
        assert_eq!(
            receiver_review_decision(&paths, "bridge", "approval").unwrap(),
            Some(ReceiverDecision::Approved)
        );
        let payload = review.as_object().unwrap();
        let start = serde_json::json!({
            "schemaVersion": PROTOCOL_VERSION,
            "bridgeId": "bridge", "planId": revision.plan_id, "revisionId": revision.revision_id,
            "revisionHash": revision.revision_hash, "requesterDeviceRef": "requester", "receiverDeviceRef": "receiver",
            "approvalId": "approval", "attemptId": "attempt", "searchStep": payload["searchStep"].clone(),
            "searchStepDigest": payload["searchStepDigest"].clone(), "attemptNonce": "attempt-nonce", "attemptExpiresAt": now + 600,
        });
        accept_inbound_protocol_event(
            &paths,
            &authorities,
            &mut candidates,
            "bridge_plan.attempt_start",
            &outer("bridge_plan.attempt_start", start, "requester", "receiver"),
            now,
        )
        .unwrap();
        let grant =
            consume_search_execution_grant(&paths, &authorities, "bridge", "attempt", now).unwrap();
        assert_eq!(grant.extensions, vec!["pdf"]);
        assert_eq!(grant.safe_scope_labels, vec!["documents"]);
        assert!(
            consume_search_execution_grant(&paths, &authorities, "bridge", "attempt", now).is_err()
        );
    }
}

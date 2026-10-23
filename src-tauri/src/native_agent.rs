//! Native mature-Agent Host capabilities.
//!
//! This module is deliberately outside the managed Worker and object-flow
//! machinery.  A native Agent owns its provider, authentication, tools,
//! sandbox, workspace semantics, and conversation.  Pastey owns only the
//! bounded task envelope and the lifecycle it needs for orchestration.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::host_identity::HostSessionBinding;

mod admission;
mod capability;
pub(crate) mod sessions;
pub(crate) use admission::NativeInvocationReviewV1;
use admission::{
    NativeAdmissionDecisionV1, NativeInvocationContinuationV1, NativeInvocationPrincipalV1,
    PendingNativeInvocationV1,
};
#[cfg(test)]
use capability::codex::codex_compatibility_at;
pub(crate) use capability::OpaqueCapabilityPayloadV1;
use capability::{
    codex::CodexNativeAdapterV1, ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeConversationV1,
    NativeInvocationOutcomeV1, NativeInvocationProgressV1,
};

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_TASK_BYTES: usize = 16 * 1024;

/// Continuing an existing Codex thread (`capability::codex::thread_continuation`).
pub(crate) const CODEX_THREAD_CAPABILITY: &str =
    capability::codex::thread_continuation::CODEX_THREAD_CAPABILITY_ID;

/// A requesting device's continuation input: the folder only as the scope
/// reference its Session reading permission showed it.
pub(crate) fn codex_thread_peer_input(
    scope_ref: &str,
    thread_id: &str,
    prompt: &str,
    expected_last_turn_id: Option<&str>,
) -> AppResult<OpaqueCapabilityPayloadV1> {
    capability::codex::thread_continuation::peer_input(
        scope_ref,
        thread_id,
        prompt,
        expected_last_turn_id,
    )
}

/// The executing Host resolves a peer's continuation input among the
/// folders that exact device may read now, `(scope_ref, workspace)`.
pub(crate) fn resolve_codex_thread_peer_input(
    input: &OpaqueCapabilityPayloadV1,
    readable: &[(String, PathBuf)],
) -> AppResult<OpaqueCapabilityPayloadV1> {
    capability::codex::thread_continuation::resolve_peer_input(input, readable)
}

/// The executing Host resolves a peer's Codex run naming a shared folder by
/// its reference (`scope:`) among the folders that exact device may ask to
/// run agents in now. `None`: the input names a path, unchanged.
pub(crate) fn resolve_codex_workspace_ref(
    input: &OpaqueCapabilityPayloadV1,
    shared: &[(String, PathBuf)],
) -> AppResult<Option<OpaqueCapabilityPayloadV1>> {
    capability::codex::resolve_peer_workspace_ref(input, shared)
}

/// One local lifecycle line per Native Agent transition, so a crash or a
/// restart leaves evidence of how far a task got. Task identities and
/// states only: never a workspace path, prompt, output or native content.
fn log_native_lifecycle(event: &str, task_id: &str, detail: &str) {
    crate::logging::write_transfer_line(&native_lifecycle_line(event, task_id, detail));
}

fn native_lifecycle_line(event: &str, task_id: &str, detail: &str) -> String {
    let separator = if detail.is_empty() { "" } else { " " };
    format!("[pastey native-agent] event={event} task={task_id}{separator}{detail}")
}

fn lifecycle_state(status: &NativeAgentTaskStatusV1) -> String {
    format!(
        "state={:?} code={}",
        status.state,
        status.code.as_deref().unwrap_or("none")
    )
}
/// Bounds one native control RPC acknowledgement only. It never bounds the
/// accepted native turn's execution or terminal observation.
const NATIVE_AGENT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const NATIVE_AGENT_PROTOCOL_SCHEMA: &str = "pastey-native-agent-control-v1";
pub(crate) const NATIVE_AGENT_TASK_SCHEMA: &str = "pastey-native-agent-task-v1";
pub(crate) const CODEX_CAPABILITY_ID: &str = "agent.coding.codex";
pub(crate) const NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA: &str =
    "pastey-native-agent-workspace-movement-v1";
/// The exact control surface of the Codex capability's direct (v1) invocation.
/// This is a Host capability fact, never execution, Transfer, or session
/// authority.
pub(crate) const DIRECT_NATIVE_INVOKE_PROTOCOLS: [&str; 2] =
    [NATIVE_AGENT_PROTOCOL_SCHEMA, NATIVE_AGENT_TASK_SCHEMA];
pub(crate) const WORKSPACE_MOVEMENT_PROTOCOLS: [&str; 3] = [
    NATIVE_AGENT_PROTOCOL_SCHEMA,
    NATIVE_AGENT_TASK_SCHEMA,
    NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA,
];
/// Invocation of any registered native capability with an opaque input.
/// Status, cancel and reconcile keep `NATIVE_AGENT_PROTOCOL_SCHEMA`.
pub(crate) const NATIVE_AGENT_INVOKE_V2_SCHEMA: &str = "pastey-native-agent-invoke-v2";
pub(crate) const GENERIC_NATIVE_INVOKE_PROTOCOLS: [&str; 2] =
    [NATIVE_AGENT_PROTOCOL_SCHEMA, NATIVE_AGENT_INVOKE_V2_SCHEMA];
/// A running task whose native capability waits for its own client to
/// answer a request (an approval, a question) that Pastey does not answer.
pub(crate) const AWAITING_NATIVE_DECISION_CODE: &str = "native_agent_awaiting_native_decision";
/// The adapter proved its invocation never started any native work.
const START_FAILED_CODE: &str = "native_agent_start_failed";
/// The owner's earlier native conversation could not be continued, so
/// nothing ran. Pastey never substitutes a new conversation for it.
pub(crate) const CONVERSATION_UNAVAILABLE_CODE: &str = "native_agent_conversation_unavailable";
const NATIVE_CONVERSATION_SCHEMA: &str = "pastey-native-conversation-v1";
const MAX_TASK_ID_BYTES: usize = 256;
const MAX_WORKSPACE_BYTES: usize = 4 * 1024;
const MAX_MOVEMENT_ID_BYTES: usize = 256;

/// Capability-neutral invocation. Core routes it by `agent_capability` and
/// never reads `input`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentInvokeV2 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) agent_capability: String,
    pub(crate) input: OpaqueCapabilityPayloadV1,
}

/// One received invocation, whichever schema carried it.
pub(crate) struct NativeInvocationRequestV1 {
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) agent_capability: String,
    pub(crate) input: OpaqueCapabilityPayloadV1,
}

/// Parses `native_agent.invoke` in either schema. The v1 schema is Codex's
/// original invocation; its fields become the Codex adapter's input.
pub(crate) fn parse_invoke(payload: Value) -> AppResult<NativeInvocationRequestV1> {
    match payload.get("schemaVersion").and_then(Value::as_str) {
        Some(NATIVE_AGENT_INVOKE_V2_SCHEMA) => {
            let request: NativeAgentInvokeV2 = serde_json::from_value(payload).map_err(|_| {
                AppError::InvalidInput("Native Agent invocation is invalid.".into())
            })?;
            validate_invoke_v2(&request)?;
            Ok(NativeInvocationRequestV1 {
                task_id: request.task_id,
                target_host_ref: request.target_host_ref,
                agent_capability: request.agent_capability,
                input: request.input,
            })
        }
        _ => {
            let request: NativeAgentInvokeV1 = serde_json::from_value(payload).map_err(|_| {
                AppError::InvalidInput("Native Agent invocation is invalid.".into())
            })?;
            validate_invoke(&request)?;
            Ok(NativeInvocationRequestV1 {
                input: capability::codex::legacy_v1_input(
                    &request.workspace,
                    &request.task,
                    request.resume,
                ),
                task_id: request.task_id,
                target_host_ref: request.target_host_ref,
                agent_capability: request.agent_capability,
            })
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentInvokeV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) agent_capability: String,
    pub(crate) workspace: String,
    pub(crate) task: String,
    pub(crate) resume: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentStatusV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) executing_host_ref: String,
    pub(crate) status: NativeAgentTaskStatusV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentCancelV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
}

/// A bounded query for durable Pastey envelope facts. It carries neither a
/// native session identifier nor any Agent-private execution detail.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentReconcileV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) movement_id: Option<String>,
    pub(crate) target_host_ref: String,
}

/// Authenticated request for the executing Host to resend only the exact
/// durable result already recorded for this movement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentRetryResultReturnV1 {
    pub(crate) schema_version: String,
    pub(crate) retry_id: String,
    pub(crate) movement_id: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentReconciliationV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) movement_id: Option<String>,
    pub(crate) executing_host_ref: String,
    pub(crate) task_state: NativeAgentTaskStateV1,
    pub(crate) movement_state: Option<NativeAgentWorkspaceMovementStateV1>,
    pub(crate) result_digest: Option<String>,
    pub(crate) apply_completed: bool,
    pub(crate) code: Option<String>,
    /// The executor's recorded terminal output, opaque to Pastey. Untrusted
    /// capability-owned data; see `NativeAgentTaskStatusV1::output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task_output: Option<OpaqueCapabilityPayloadV1>,
}

/// Immutable control input sent before an existing encrypted Transfer carries
/// an approved workspace to the Agent Host.  It contains no source path,
/// credential, provider, session identifier, or managed-object binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentWorkspacePrepareV1 {
    pub(crate) schema_version: String,
    pub(crate) movement_id: String,
    pub(crate) task_id: String,
    pub(crate) source_host_ref: String,
    pub(crate) target_host_ref: String,
    pub(crate) agent_capability: String,
    pub(crate) task: String,
    pub(crate) resume: bool,
    pub(crate) source_object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
    pub(crate) source_digest: String,
    pub(crate) source_bytes: u64,
}

/// Transfer metadata for the existing encrypted file transport.  The two
/// phases are authored as one approved envelope: source-to-Agent and result
/// back to source.  This is correlation data, not a new Transfer primitive.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentWorkspaceTransferPhaseV1 {
    Outbound,
    Return,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentWorkspaceTransferV1 {
    pub(crate) schema_version: String,
    pub(crate) movement_id: String,
    pub(crate) task_id: String,
    pub(crate) phase: NativeAgentWorkspaceTransferPhaseV1,
    pub(crate) bridge_id: String,
    pub(crate) source_host_ref: String,
    pub(crate) destination_host_ref: String,
    pub(crate) object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
    pub(crate) content_digest: String,
    pub(crate) logical_byte_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentWorkspaceMovementStateV1 {
    Review,
    AwaitingApproval,
    TransferringToAgent,
    AgentRunning,
    ReturningResult,
    ApplyingResult,
    Completed,
    ConflictRecoveryRequired,
    Failed,
    Cancelled,
    Interrupted,
}

/// Review drafts do not own their source. Exactly one post-approval movement
/// may own a canonical source workspace until it reaches a non-authoritative
/// terminal state.
fn movement_holds_source_ownership(movement: &NativeAgentWorkspaceMovementV1) -> bool {
    matches!(
        movement.state,
        NativeAgentWorkspaceMovementStateV1::TransferringToAgent
            | NativeAgentWorkspaceMovementStateV1::AgentRunning
            | NativeAgentWorkspaceMovementStateV1::ReturningResult
            | NativeAgentWorkspaceMovementStateV1::ApplyingResult
    ) || (movement.state == NativeAgentWorkspaceMovementStateV1::Interrupted
        && matches!(
            movement.code.as_deref(),
            Some("native_agent_reconciliation_required")
                | Some("native_agent_outcome_unknown")
                | Some("result_apply_interrupted")
                | Some("conflict_result_retention_required")
                | Some("conflict_result_retention_failed")
                | Some("native_agent_result_snapshot_recovery_failed")
        ))
}

fn task_accepts_remote_fact(
    current: &NativeAgentTaskStatusV1,
    next: &NativeAgentTaskStatusV1,
) -> bool {
    use NativeAgentTaskStateV1::*;
    // A recorded terminal output is the capability's authoritative result;
    // no later fact may replace it.
    if current.output.is_some() && next.output != current.output {
        return false;
    }
    // A task never changes capability.
    if next.agent_id != current.agent_id {
        return false;
    }
    match current.state {
        Cancelled => next.state == Cancelled,
        Completed => next.state == Completed,
        Failed => matches!(next.state, Failed | Cancelled),
        Running => next.state != Queued,
        Interrupted
            if !matches!(
                current.code.as_deref(),
                Some("native_agent_reconciliation_required") | Some("native_agent_outcome_unknown")
            ) =>
        {
            matches!(next.state, Interrupted | Cancelled)
        }
        Interrupted => matches!(next.state, Completed | Failed | Cancelled),
        Queued => true,
    }
}

fn movement_accepts_remote_execution_fact(
    record: &WorkspaceMovementRecordV1,
    next: &NativeAgentWorkspaceMovementStateV1,
) -> bool {
    use NativeAgentWorkspaceMovementStateV1::*;
    if record.apply_completed {
        return false;
    }
    match record.status.state {
        Cancelled | Completed | ConflictRecoveryRequired | Failed => false,
        ReturningResult | ApplyingResult => false,
        Interrupted
            if record.status.code.as_deref() != Some("native_agent_reconciliation_required") =>
        {
            false
        }
        Review | AwaitingApproval => false,
        TransferringToAgent => !matches!(next, Review | AwaitingApproval),
        AgentRunning => !matches!(next, Review | AwaitingApproval | TransferringToAgent),
        // A requester-local reconciliation requirement is stronger than an
        // older executing-Host observation. Only the explicit reconciliation
        // path may close it, and only with an apply-completed fact.
        Interrupted => false,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentWorkspaceMovementV1 {
    pub(crate) schema_version: String,
    pub(crate) movement_id: String,
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) source_workspace_name: String,
    pub(crate) target_host_ref: String,
    pub(crate) review_summary: String,
    pub(crate) state: NativeAgentWorkspaceMovementStateV1,
    pub(crate) code: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct LocalMovementSourceV1 {
    workspace: PathBuf,
    baseline: crate::safe_file_identity::RegularFileSetIdentity,
    object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
    task: String,
    resume: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct PreparedRemoteWorkspaceV1 {
    source_host_ref: String,
    task: String,
    resume: bool,
    source_object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
    source_digest: String,
    source_bytes: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct WorkspaceMovementRecordV1 {
    status: NativeAgentWorkspaceMovementV1,
    source: Option<LocalMovementSourceV1>,
    prepared_remote: Option<PreparedRemoteWorkspaceV1>,
    task_workspace: Option<PathBuf>,
    /// Stable Host-private result tree captured before any return delivery.
    result_snapshot: Option<PathBuf>,
    result_identity: Option<crate::safe_file_identity::RegularFileSetIdentity>,
    /// Executor-proven digest for a Return not yet received by the requester.
    /// This is never a requester-side result identity or apply receipt.
    #[serde(default)]
    expected_return_digest: Option<String>,
    bridge_id: Option<String>,
    apply_completed: bool,
    #[serde(default)]
    apply_transaction: Option<WorkspaceApplyTransactionV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceApplyPhaseV1 {
    Staging,
    Staged,
    OriginalMoved,
    ResultInstalled,
    Committed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplyCrashPointV1 {
    IntentPersisted,
    StageWritten,
    StageJournaled,
    OriginalMoved,
    OriginalMoveJournaled,
    ResultInstalled,
    ResultInstallJournaled,
    Committed,
    BackupCleaned,
}

fn simulate_apply_crash(
    requested: Option<ApplyCrashPointV1>,
    boundary: ApplyCrashPointV1,
) -> AppResult<()> {
    if requested == Some(boundary) {
        return invalid("simulated Native Agent apply process crash");
    }
    Ok(())
}

/// Durable intent for replacing the source workspace. Stage and backup names
/// are derived from the immutable movement id, so every sibling directory is
/// correlated to this record before any rename can make the source absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct WorkspaceApplyTransactionV1 {
    result_identity: crate::safe_file_identity::RegularFileSetIdentity,
    phase: WorkspaceApplyPhaseV1,
}

/// The authority a task belongs to. Once a task exists its owner never
/// changes: no invocation, replay, preparation, cancellation or failure from
/// another principal may adopt, move or erase it. Only Burn removes a
/// Bridge-owned task, together with its owner. The owner is durable as the
/// envelope's `bridge_id` (or its movement's), and its absence means Local.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TaskOwnerV1 {
    Local,
    Bridge(String),
}

#[derive(Clone, Deserialize, Serialize)]
struct PersistedNativeAgentEnvelopeV1 {
    task: NativeAgentTaskStatusV1,
    task_workspace: Option<PathBuf>,
    remote_target: Option<String>,
    task_digest: Option<String>,
    #[serde(default)]
    bridge_id: Option<String>,
    movement: Option<WorkspaceMovementRecordV1>,
    /// Held while the task's outcome is unresolved, also across restart.
    #[serde(default)]
    exclusivity_key: Option<ExclusivityKeyV1>,
}

/// The deliberately small renderer-safe projection used only to reopen
/// unresolved Bridge-bound work after restart. It contains no physical path,
/// native session/process identifier, provider/auth state, or Agent history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeAgentRecoveryTaskProjectionV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) workspace_name: String,
    pub(crate) state: NativeAgentTaskStateV1,
    pub(crate) code: Option<String>,
}

impl From<&NativeAgentTaskStatusV1> for NativeAgentRecoveryTaskProjectionV1 {
    fn from(task: &NativeAgentTaskStatusV1) -> Self {
        Self {
            schema_version: task.schema_version.clone(),
            task_id: task.task_id.clone(),
            agent_id: task.agent_id.clone(),
            workspace_name: task.workspace_name.clone(),
            state: task.state.clone(),
            code: task.code.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeAgentRecoveryProjectionV1 {
    pub(crate) task: NativeAgentRecoveryTaskProjectionV1,
    pub(crate) movement: Option<NativeAgentWorkspaceMovementV1>,
    pub(crate) target_host_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentTaskStateV1 {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

/// Host-private terminal classification. It deliberately contains no native
/// session, provider, or process detail and is not a generic Agent framework.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeTurnOutcomeV1 {
    Completed,
    Failed,
    Interrupted,
    Cancelled,
    Unknown,
    /// Nothing native ran: the capability could not even begin the
    /// invocation. A definite non-start, never an indeterminate outcome.
    NotStarted,
    /// Nothing native ran: the conversation the invocation continues could
    /// not be reopened.
    ConversationUnavailable,
}

/// How long the requesting user has to decide a native request remotely.
pub(crate) const NATIVE_DECISION_REVIEW_TTL_SECONDS: i64 = 30 * 60;
/// Most native decision reviews one task carries at once.
pub(crate) const MAX_NATIVE_DECISION_REVIEWS: usize = 3;

/// One Bridge's native capability Reviews on this Host, by outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeReviewCountsV1 {
    pub(crate) pending: usize,
    pub(crate) limit: usize,
    pub(crate) started: usize,
    pub(crate) denied: usize,
    pub(crate) expired: usize,
    pub(crate) revoked: usize,
}
/// Bound for one review's material content, as JSON. Larger content is
/// never summarized or cut: the review simply cannot be approved. With
/// `MAX_NATIVE_DECISION_REVIEWS` it keeps a status within one control event.
pub(crate) const MAX_NATIVE_DECISION_CONTENT_BYTES: usize = 10 * 1024;

/// What a native capability asks its client to decide.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeDecisionKindV1 {
    /// Run one command.
    CommandExecution,
    /// Apply one set of file changes.
    FileChange,
    /// Something Pastey does not relay (a permission grant, a question, an
    /// MCP form, terminal input). Visible and stoppable, never answered.
    Unsupported,
}

/// A human's answer to one native request. Only one-time answers exist:
/// nothing is approved for a session, a turn or by policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeDecisionV1 {
    /// Allow exactly this one operation.
    Accept,
    /// Refuse it; the native task continues without it.
    Decline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeDecisionStateV1 {
    /// Waiting for the requesting user.
    Pending,
    /// The user's answer was given to the native capability; it has not yet
    /// reported the request resolved.
    Submitted,
    /// The native capability reported the request resolved (answered, or
    /// cleared by its own turn change).
    Resolved,
}

/// The material, native-original content of one request. Untrusted data,
/// displayed only as escaped text.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeDecisionContentV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) network_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) network_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) file_changes: Vec<NativeFileChangeV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeFileChangeV1 {
    pub(crate) path: String,
    /// `add`, `delete`, `update`, or `move` (with `moveTo`).
    pub(crate) change: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) move_to: Option<String>,
    pub(crate) diff: String,
}

/// One native request awaiting its requesting user's decision, as both Hosts
/// see it. `review_ref` is Pastey's opaque reference; native request, thread
/// and turn identifiers never leave the executing Host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeDecisionReviewV1 {
    pub(crate) review_ref: String,
    pub(crate) kind: NativeDecisionKindV1,
    pub(crate) state: NativeDecisionStateV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) submitted: Option<NativeDecisionV1>,
    pub(crate) can_accept: bool,
    pub(crate) can_decline: bool,
    /// Why approval (or any answer) is not offered, as a lifecycle code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unavailable_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<NativeDecisionContentV1>,
    /// Digest of exactly the kind and content shown; a decision names it.
    pub(crate) content_digest: String,
    pub(crate) requested_at: i64,
    pub(crate) expires_at: i64,
}

pub(crate) fn native_decision_content_digest(
    kind: NativeDecisionKindV1,
    content: Option<&NativeDecisionContentV1>,
) -> String {
    let bytes =
        serde_json::to_vec(&json!({ "kind": kind, "content": content })).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

fn validate_native_decision_review(review: &NativeDecisionReviewV1) -> bool {
    review.review_ref.starts_with("native-decision:")
        && review.review_ref.len() <= 96
        && review
            .review_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':'))
        && review.content_digest.len() == 64
        && review
            .content_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && valid_lifecycle_code(review.unavailable_reason.as_deref())
        && review.content.as_ref().is_none_or(|content| {
            serde_json::to_vec(content)
                .is_ok_and(|bytes| bytes.len() <= MAX_NATIVE_DECISION_CONTENT_BYTES)
        })
        && review.content_digest
            == native_decision_content_digest(review.kind, review.content.as_ref())
        && !(review.can_accept
            && (review.content.is_none() || review.kind == NativeDecisionKindV1::Unsupported))
}

/// A requesting user's explicit decision on one native request of their
/// task, sent over the authenticated Bridge. It carries no command, path or
/// native identifier: only Pastey's reference and the digest of what was
/// shown.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentDecisionV1 {
    pub(crate) schema_version: String,
    /// One attempt to decide; its replay key. Once-only answering is the
    /// executing Host's, per native request.
    pub(crate) decision_id: String,
    pub(crate) task_id: String,
    pub(crate) target_host_ref: String,
    pub(crate) review_ref: String,
    pub(crate) content_digest: String,
    pub(crate) decision: NativeDecisionV1,
}

pub(crate) fn validate_decision(request: &NativeAgentDecisionV1) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || !valid_task_id(&request.task_id)
        || request.decision_id.is_empty()
        || request.decision_id.len() > 64
        || !request
            .decision_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
        || !request.review_ref.starts_with("native-decision:")
        || request.review_ref.len() > 96
        || request.content_digest.len() != 64
        || !request
            .content_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return invalid("Native Agent decision is invalid.");
    }
    Ok(())
}

/// Whether a task continued its owner's earlier native conversation or
/// began a new one. Renderer-safe: no native identifier.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeConversationContinuityV1 {
    New,
    Continued,
}

/// Who a native conversation belongs to. A conversation is continued only
/// for exactly this principal: this Host's own user, or one authenticated
/// peer Host over one Bridge. A workspace path alone never grants it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NativeConversationOwnerV1 {
    Local,
    Peer {
        bridge_id: String,
        peer_host_ref: String,
    },
}

impl NativeConversationOwnerV1 {
    fn bridge_id(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Peer { bridge_id, .. } => Some(bridge_id),
        }
    }
}

/// The durable, Host-private record of one confirmed native conversation.
/// Its lifecycle is the conversation's, not any task envelope's.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeConversationRecordV1 {
    schema_version: String,
    owner: NativeConversationOwnerV1,
    capability_id: String,
    scope_digest: String,
    continuation: OpaqueCapabilityPayloadV1,
}

/// One owner's conversation in one adapter scope.
#[derive(Clone)]
struct NativeConversationBindingV1 {
    key: String,
    owner: NativeConversationOwnerV1,
    capability_id: String,
    scope_digest: String,
}

impl NativeConversationBindingV1 {
    fn new(owner: NativeConversationOwnerV1, capability_id: &str, scope: &str) -> AppResult<Self> {
        let scope_digest = blake3::hash(scope.as_bytes()).to_hex().to_string();
        let identity = serde_json::to_vec(&json!({
            "schemaVersion": NATIVE_CONVERSATION_SCHEMA,
            "owner": owner,
            "capabilityId": capability_id,
            "scopeDigest": scope_digest,
        }))?;
        Ok(Self {
            key: blake3::hash(&identity).to_hex().to_string(),
            owner,
            capability_id: capability_id.into(),
            scope_digest,
        })
    }

    /// The saved continuation of exactly this binding, if any.
    fn saved_continuation(
        &self,
        paths: &crate::storage::AppPaths,
    ) -> AppResult<Option<OpaqueCapabilityPayloadV1>> {
        let Some(record) = crate::storage::get_native_agent_conversation(paths, &self.key)? else {
            return Ok(None);
        };
        let record: NativeConversationRecordV1 = serde_json::from_str(&record)?;
        Ok((record.schema_version == NATIVE_CONVERSATION_SCHEMA
            && record.owner == self.owner
            && record.capability_id == self.capability_id
            && record.scope_digest == self.scope_digest
            && record.continuation.validate().is_ok())
        .then_some(record.continuation))
    }

    fn save(
        &self,
        paths: &crate::storage::AppPaths,
        continuation: OpaqueCapabilityPayloadV1,
    ) -> AppResult<bool> {
        continuation.validate()?;
        let record = NativeConversationRecordV1 {
            schema_version: NATIVE_CONVERSATION_SCHEMA.into(),
            owner: self.owner.clone(),
            capability_id: self.capability_id.clone(),
            scope_digest: self.scope_digest.clone(),
            continuation,
        };
        crate::storage::save_native_agent_conversation(
            paths,
            &self.key,
            self.owner.bridge_id(),
            &serde_json::to_string(&record)?,
        )
    }
}

/// Core's receiver for one observed invocation's non-terminal facts.
struct ObservedTaskProgressV1 {
    tasks: Arc<Mutex<HashMap<String, NativeAgentTaskStatusV1>>>,
    task_id: String,
    /// Whether a proven requesting device may decide this task's native
    /// requests. Otherwise none is offered: nobody could give the answer.
    remote_decisions: bool,
    conversation: Option<NativeConversationBindingV1>,
    durable_paths: Option<crate::storage::AppPaths>,
}

impl NativeInvocationProgressV1 for ObservedTaskProgressV1 {
    fn awaiting_native_decision(&self, waiting: bool) {
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        let Some(status) = tasks.get_mut(&self.task_id) else {
            return;
        };
        // Only a running task carries it: cancellation and every terminal
        // fact already recorded win over a late progress report.
        if status.state != NativeAgentTaskStateV1::Running {
            return;
        }
        if waiting {
            status.code = Some(AWAITING_NATIVE_DECISION_CODE.into());
        } else if status.code.as_deref() == Some(AWAITING_NATIVE_DECISION_CODE) {
            status.code = None;
        }
    }

    fn native_decision_updated(&self, mut review: NativeDecisionReviewV1) {
        if !self.remote_decisions && (review.can_accept || review.can_decline) {
            review.can_accept = false;
            review.can_decline = false;
            review.unavailable_reason = Some("native_decision_requester_unverified".into());
        }
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        let Some(status) = tasks
            .get_mut(&self.task_id)
            .filter(|status| status.state == NativeAgentTaskStateV1::Running)
        else {
            return;
        };
        match status
            .native_decisions
            .iter_mut()
            .find(|known| known.review_ref == review.review_ref)
        {
            Some(known) => *known = review,
            None => {
                // Resolved reviews give way first; an unrepresentable
                // surplus is simply not offered (the native request still
                // waits, visibly, and can be stopped).
                if status.native_decisions.len() >= MAX_NATIVE_DECISION_REVIEWS {
                    if let Some(index) = status
                        .native_decisions
                        .iter()
                        .position(|known| known.state == NativeDecisionStateV1::Resolved)
                    {
                        status.native_decisions.remove(index);
                    }
                }
                if status.native_decisions.len() < MAX_NATIVE_DECISION_REVIEWS {
                    status.native_decisions.push(review);
                }
            }
        }
    }

    fn native_decision_resolved(&self, review_ref: &str) {
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        if let Some(review) = tasks.get_mut(&self.task_id).and_then(|status| {
            status
                .native_decisions
                .iter_mut()
                .find(|review| review.review_ref == review_ref)
        }) {
            review.state = NativeDecisionStateV1::Resolved;
            review.can_accept = false;
            review.can_decline = false;
        }
    }

    fn conversation_established(
        &self,
        continuity: NativeConversationContinuityV1,
        continuation: OpaqueCapabilityPayloadV1,
    ) {
        // The native conversation exists whatever becomes of this task, so
        // it is kept even if the task was cancelled meanwhile. A burned
        // Bridge's tombstone refuses it inside the same storage transaction.
        if let (Some(binding), Some(paths)) = (&self.conversation, &self.durable_paths) {
            if let Err(error) = binding.save(paths, continuation) {
                crate::logging::write_error_line(&format!(
                    "Native Agent conversation record was not saved: {}",
                    error.message()
                ));
            }
        }
        let Ok(mut tasks) = self.tasks.lock() else {
            return;
        };
        if let Some(status) = tasks
            .get_mut(&self.task_id)
            .filter(|status| status.state == NativeAgentTaskStateV1::Running)
        {
            status.conversation = Some(continuity);
            status.session_reused = continuity == NativeConversationContinuityV1::Continued;
        }
    }
}

/// The registered adapters, detached from the service lock so observing
/// availability (which may launch bounded probe processes) never holds it.
pub(crate) struct NativeCapabilityRegistryV1(Vec<Arc<dyn NativeCapabilityAdapterV1>>);

impl NativeCapabilityRegistryV1 {
    pub(crate) fn capabilities(&self) -> Vec<NativeAgentCapabilityV1> {
        self.0.iter().map(|adapter| adapter.describe()).collect()
    }

    /// One existing capability fact per registered native capability.
    pub(crate) fn native_capability_facts(
        &self,
    ) -> Vec<crate::peer_capabilities::HostCapabilityFact> {
        self.0
            .iter()
            // Local-only capabilities are not described to peers at all.
            .filter(|adapter| !adapter.supported_protocols().is_empty())
            .map(|adapter| {
                let report = adapter.availability_report();
                crate::peer_capabilities::native_agent_capability_fact(
                    adapter.capability_id(),
                    report.state,
                    report.reason,
                    adapter.supported_protocols(),
                )
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentCapabilityStateV1 {
    Available,
    Incompatible,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentCapabilityV1 {
    pub(crate) agent_id: String,
    pub(crate) display_name: String,
    pub(crate) state: NativeAgentCapabilityStateV1,
    /// Why it is not available, as a reason code a peer may also see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    /// What this Host's probe found, for this Host's own user only. Never
    /// sent to a peer: it may name local paths and program output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// This Host only: which installation provides it (`cli` or `desktop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) backend: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentTaskStatusV1 {
    pub(crate) schema_version: String,
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) workspace_name: String,
    pub(crate) session_reused: bool,
    pub(crate) state: NativeAgentTaskStateV1,
    pub(crate) result: Option<String>,
    pub(crate) code: Option<String>,
    /// Terminal output of a capability that reports one, opaque to Pastey.
    ///
    /// Trust boundary: this is untrusted, capability-owned data. It reaches
    /// the renderer through every IPC command that returns this status.
    /// Neither Core nor the renderer may interpret it as HTML, code, a
    /// command, a path or a URL to act on; UI consumers treat it only as
    /// data (for example, escaped text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<OpaqueCapabilityPayloadV1>,
    /// Set once the capability confirms whether this task continued its
    /// owner's conversation or began a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) conversation: Option<NativeConversationContinuityV1>,
    /// Native requests of this running task awaiting its requesting user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) native_decisions: Vec<NativeDecisionReviewV1>,
}

/// A small Host-private service: the native capability lifecycle over a
/// fixed set of registered adapters. It is not a discoverable Agent
/// marketplace, an authority source, or a Worker adapter.
/// Who asked for one Bridge task: a device key proven over a session of
/// that Bridge, from that peer Host.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeTaskRequesterV1 {
    bridge_id: String,
    peer_host_ref: String,
    fingerprint: String,
}

pub(crate) struct NativeAgentServiceV1 {
    /// Capabilities this Host can run, dispatched by capability id.
    adapters: Vec<Arc<dyn NativeCapabilityAdapterV1>>,
    /// Workspace movement's typed alias of the registered Codex adapter. It
    /// owns nothing: `adapters` owns every adapter's lifecycle.
    codex: Arc<CodexNativeAdapterV1>,
    tasks: Arc<Mutex<HashMap<String, NativeAgentTaskStatusV1>>>,
    /// Exclusivity keys held by observed invocations, with their task.
    active_exclusivity: Arc<Mutex<HashMap<ExclusivityKeyV1, String>>>,
    /// Tasks whose native observer is still running.
    observed_tasks: Arc<Mutex<HashSet<String>>>,
    remote_targets: HashMap<String, String>,
    task_workspaces: HashMap<String, PathBuf>,
    task_digests: HashMap<String, String>,
    task_exclusivity: HashMap<String, ExclusivityKeyV1>,
    /// The Bridge that owns each Bridge-derived task. A task in `tasks`
    /// without an entry here is Local. See `TaskOwnerV1`.
    task_bridges: HashMap<String, String>,
    revoked_bridges: HashSet<String>,
    /// Remote invocations held for Host Review, by task. Process-local and
    /// one-shot; see `admission`.
    pending_reviews: HashMap<String, PendingNativeInvocationV1>,
    /// For a Review whose invocation came through a shared folder: the
    /// share, which its Accept checks again before starting anything.
    review_share_requirements: HashMap<String, crate::session_read_access::ShareRequirementV1>,
    /// Tasks a Review created that never started. No adapter has seen them
    /// and they have no durable envelope, so a restart forgets them.
    unstarted_review_tasks: HashSet<String>,
    /// Digest of the exact input Core admitted for a task. Replays compare it
    /// so a replayed remote input never reaches an adapter.
    task_input_digests: HashMap<String, String>,
    /// The exact authenticated peer session each received workspace movement
    /// was prepared over: the principal of its executor Review once the
    /// workspace lands. Process-local, like the Review itself.
    movement_bindings: HashMap<String, HostSessionBinding>,
    /// The conversation key each Bridge-owned task ran in, so Burn can end
    /// that conversation's live native state.
    task_conversations: HashMap<String, String>,
    /// The device, proven in its session, that requested each Bridge task:
    /// the only one whose user may decide that task's native requests.
    /// Process-local; a restart never resumes a native turn.
    task_requesters: HashMap<String, NativeTaskRequesterV1>,
    workspace_movements: HashMap<String, WorkspaceMovementRecordV1>,
    durable_paths: Option<crate::storage::AppPaths>,
    #[cfg(test)]
    fail_post_start_persist_once: bool,
}

impl Default for NativeAgentServiceV1 {
    fn default() -> Self {
        let codex = Arc::new(CodexNativeAdapterV1::default());
        #[cfg_attr(test, allow(unused_mut))]
        let mut adapters: Vec<Arc<dyn NativeCapabilityAdapterV1>> = vec![codex.clone()];
        // Claude Code session continuation (local only). Tests register it
        // explicitly with a fixture, never the installed `claude`.
        #[cfg(not(test))]
        adapters.push(Arc::new(
            capability::claude_code::ClaudeCodeNativeAdapterV1::default(),
        ));
        // Continuing an existing Codex thread under Codex's own writer lock
        // (local only). Tests register it explicitly with a fixture.
        #[cfg(not(test))]
        adapters.push(Arc::new(
            capability::codex::thread_continuation::CodexThreadAdapterV1::sharing(
                codex.own_thread_holds(),
            ),
        ));
        Self::with_adapters(codex, adapters)
    }
}

impl NativeAgentServiceV1 {
    fn with_adapters(
        codex: Arc<CodexNativeAdapterV1>,
        adapters: Vec<Arc<dyn NativeCapabilityAdapterV1>>,
    ) -> Self {
        Self {
            adapters,
            codex,
            tasks: Arc::default(),
            active_exclusivity: Arc::default(),
            observed_tasks: Arc::default(),
            remote_targets: HashMap::new(),
            task_workspaces: HashMap::new(),
            task_digests: HashMap::new(),
            task_exclusivity: HashMap::new(),
            task_bridges: HashMap::new(),
            revoked_bridges: HashSet::new(),
            pending_reviews: HashMap::new(),
            review_share_requirements: HashMap::new(),
            unstarted_review_tasks: HashSet::new(),
            task_input_digests: HashMap::new(),
            movement_bindings: HashMap::new(),
            task_conversations: HashMap::new(),
            task_requesters: HashMap::new(),
            workspace_movements: HashMap::new(),
            durable_paths: None,
            #[cfg(test)]
            fail_post_start_persist_once: false,
        }
    }

    pub(crate) fn with_paths(paths: crate::storage::AppPaths) -> AppResult<Self> {
        let mut service = Self {
            durable_paths: Some(paths),
            ..Self::default()
        };
        service.restore_durable_envelopes()?;
        #[cfg(not(test))]
        {
            // Observe availability once in the background so the first
            // capability query or invocation does not pay for the probe.
            let registry = service.capability_registry();
            thread::spawn(move || {
                let _ = registry.capabilities();
            });
        }
        Ok(service)
    }

    /// A Host whose only native capabilities are `adapters` (Codex absent).
    #[cfg(test)]
    fn with_paths_and_adapters(
        paths: crate::storage::AppPaths,
        adapters: Vec<Arc<dyn NativeCapabilityAdapterV1>>,
    ) -> AppResult<Self> {
        let mut service = Self::with_adapters(Arc::new(CodexNativeAdapterV1::default()), adapters);
        service.durable_paths = Some(paths);
        service.restore_durable_envelopes()?;
        Ok(service)
    }

    /// Tests: registers the Codex thread continuation adapter (registered
    /// only in the product otherwise) with a fixture Codex and Codex home.
    #[cfg(test)]
    pub(crate) fn add_codex_thread_adapter_for_test(
        &mut self,
        executable: &Path,
        codex_home: &Path,
    ) {
        let adapter = capability::codex::thread_continuation::CodexThreadAdapterV1::sharing(
            self.codex.own_thread_holds(),
        );
        adapter.use_executable(executable);
        adapter.use_codex_home(codex_home);
        self.adapters.push(Arc::new(adapter));
    }

    /// Tests: the task occupying a canonical Codex workspace, if any.
    #[cfg(test)]
    fn workspace_occupant(&self, workspace: &Path) -> Option<String> {
        self.active_exclusivity
            .lock()
            .unwrap()
            .get(&capability::codex::workspace_exclusivity_key(workspace))
            .cloned()
    }

    fn adapter(&self, capability_id: &str) -> AppResult<Arc<dyn NativeCapabilityAdapterV1>> {
        self.adapters
            .iter()
            .find(|adapter| adapter.capability_id() == capability_id)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidInput("Native capability is unavailable on this Host.".into())
            })
    }

    /// The owner of an existing task, or `None` when no such task exists.
    fn task_owner(&self, task_id: &str) -> AppResult<Option<TaskOwnerV1>> {
        if !self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .contains_key(task_id)
        {
            return Ok(None);
        }
        Ok(Some(match self.task_bridges.get(task_id) {
            Some(bridge_id) => TaskOwnerV1::Bridge(bridge_id.clone()),
            None => TaskOwnerV1::Local,
        }))
    }

    /// Runs `create` for a task owned by `bridge_id`. An existing task with
    /// another owner is refused before anything is touched; an existing task
    /// of this Bridge reaches `create`'s own replay checks with its ownership
    /// untouched. A new task's owner is recorded first so its durable
    /// envelope carries it, and is released only if no task was created.
    fn with_bridge_task_owner<T>(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        create: impl FnOnce(&mut Self) -> AppResult<T>,
    ) -> AppResult<T> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        match self.task_owner(task_id)? {
            Some(TaskOwnerV1::Bridge(owner)) if owner == bridge_id => create(self),
            Some(_) => invalid("Native Agent task belongs to another authority."),
            None => {
                self.task_bridges
                    .insert(task_id.to_owned(), bridge_id.to_owned());
                let result = create(self);
                if result.is_err() && self.task_owner(task_id)?.is_none() {
                    self.task_bridges.remove(task_id);
                }
                result
            }
        }
    }

    /// Local starts never join a task that a Bridge owns.
    fn require_local_task_owner(&self, task_id: &str) -> AppResult<()> {
        if self.task_bridges.contains_key(task_id) {
            return invalid("Native Agent task belongs to another authority.");
        }
        Ok(())
    }

    /// Loads only Pastey's outer facts. Any task whose native outcome was not
    /// durably observed is intentionally interrupted: a restart never restores
    /// a Codex process, session, turn, or retry authority.
    fn restore_durable_envelopes(&mut self) -> AppResult<()> {
        let Some(paths) = self.durable_paths.clone() else {
            return Ok(());
        };
        let mut pending_conflict_discards = Vec::new();
        for stored in crate::storage::list_native_agent_envelopes(&paths)? {
            let mut persisted: PersistedNativeAgentEnvelopeV1 =
                serde_json::from_str(&stored.record_json).map_err(AppError::from)?;
            // A restart never resumes a native turn, nor anything it asked.
            let mut changed = !persisted.task.native_decisions.is_empty();
            persisted.task.native_decisions.clear();
            if persisted.task.state == NativeAgentTaskStateV1::Queued
                && persisted.task.code.as_deref() == Some(admission::REVIEW_REQUIRED_CODE)
            {
                // A Review is process-local and never survives restart. Its
                // task never started, so this is a definite non-start, not a
                // reconciliation; a landed workspace is cleaned below.
                persisted.task.state = NativeAgentTaskStateV1::Failed;
                persisted.task.code = Some(admission::ADMISSION_REVOKED_CODE.into());
                changed = true;
            }
            let unbound_local_restart = persisted.bridge_id.is_none()
                && persisted.remote_target.is_none()
                && persisted.movement.is_none()
                && (matches!(
                    persisted.task.state,
                    NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
                ) || (persisted.task.state == NativeAgentTaskStateV1::Interrupted
                    && persisted.task.code.as_deref()
                        == Some("native_agent_reconciliation_required")));
            if matches!(
                persisted.task.state,
                NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
            ) {
                if unbound_local_restart {
                    persisted.task.state = NativeAgentTaskStateV1::Failed;
                    persisted.task.code = Some("native_agent_interrupted_on_restart".into());
                } else {
                    persisted.task.state = NativeAgentTaskStateV1::Interrupted;
                    persisted.task.code = Some("native_agent_reconciliation_required".into());
                }
                // A turn this process was running did not survive the restart;
                // whether the native Agent finished it is not known here.
                log_native_lifecycle(
                    "interrupted_by_restart",
                    &persisted.task.task_id,
                    &lifecycle_state(&persisted.task),
                );
                changed = true;
            } else if unbound_local_restart {
                persisted.task.state = NativeAgentTaskStateV1::Failed;
                persisted.task.code = Some("native_agent_interrupted_on_restart".into());
                changed = true;
            }
            if let Some(movement) = persisted.movement.as_mut() {
                if movement.apply_transaction.is_some() {
                    let movement_id = movement.status.movement_id.clone();
                    match recover_workspace_apply_record(&movement_id, movement) {
                        Ok(()) => changed = true,
                        Err(_) => {
                            movement.status.state =
                                NativeAgentWorkspaceMovementStateV1::Interrupted;
                            movement.status.code = Some("result_apply_interrupted".into());
                            changed = true;
                        }
                    }
                }
                let durable_conflict = movement.result_identity.as_ref().is_some_and(|identity| {
                    crate::storage::get_native_agent_conflict(&paths, &movement.status.movement_id)
                        .ok()
                        .flatten()
                        .is_some_and(|conflict| {
                            conflict.task_id == movement.status.task_id
                                && conflict.result_digest == identity.digest
                                && conflict.result_byte_count == identity.byte_count
                                && validate_retained_conflict(&paths, &conflict).is_ok()
                        })
                });
                if movement.status.state
                    == NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                    && !durable_conflict
                {
                    // Never advertise a recoverable conflict whose durable
                    // retained result cannot be proved after restart.
                    movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                    movement.status.code = Some("conflict_result_retention_required".into());
                    changed = true;
                } else if movement.status.state
                    == NativeAgentWorkspaceMovementStateV1::ApplyingResult
                    && movement.status.code.as_deref() == Some("conflict_result_retention_pending")
                    && durable_conflict
                {
                    // Retention reached durable storage before the terminal
                    // envelope write. Finish only the outer terminal fact;
                    // never redo Agent work or create another retained tree.
                    movement.status.state =
                        NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired;
                    movement.status.code = Some("source_changed_since_approval".into());
                    changed = true;
                } else if movement.status.state
                    == NativeAgentWorkspaceMovementStateV1::ApplyingResult
                    && movement.status.code.as_deref() == Some("conflict_result_retention_pending")
                {
                    // The exact returned digest was durable but retention did
                    // not complete. A matching Return may repair this outer
                    // consequence; execution itself is never retried.
                    movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                    movement.status.code = Some("conflict_result_retention_required".into());
                    changed = true;
                } else if movement.status.state == NativeAgentWorkspaceMovementStateV1::Interrupted
                    && matches!(
                        movement.status.code.as_deref(),
                        Some("result_apply_interrupted")
                            | Some("conflict_result_retention_required")
                            | Some("conflict_result_retention_failed")
                    )
                {
                    // Preserve exact local consequence recovery across every
                    // restart; generic execution reconciliation cannot accept
                    // an already received Return identity.
                } else if persisted.task.state == NativeAgentTaskStateV1::Completed
                    && movement.prepared_remote.is_some()
                    && movement.apply_transaction.is_none()
                    && !movement.apply_completed
                    && !matches!(
                        movement.status.state,
                        NativeAgentWorkspaceMovementStateV1::Completed
                            | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                            | NativeAgentWorkspaceMovementStateV1::Failed
                            | NativeAgentWorkspaceMovementStateV1::Cancelled
                    )
                {
                    // Completion is durable execution evidence. If Pastey
                    // crashed before the Return snapshot was sealed, recover
                    // only that consequence from the retained received tree;
                    // never restore or rerun the Agent session.
                    if movement.task_workspace.is_none() {
                        movement.task_workspace = persisted.task_workspace.clone();
                        changed |= movement.task_workspace.is_some();
                    }
                    let exact_snapshot_is_valid = movement
                        .result_snapshot
                        .as_ref()
                        .zip(movement.result_identity.as_ref())
                        .is_some_and(|(snapshot, identity)| {
                            validate_exact_app_owned_result(&paths, snapshot, identity).is_ok()
                        });
                    if exact_snapshot_is_valid {
                        if movement.status.state
                            != NativeAgentWorkspaceMovementStateV1::ReturningResult
                            || movement.status.code.as_deref()
                                != Some("result_return_retry_required")
                        {
                            movement.status.state =
                                NativeAgentWorkspaceMovementStateV1::ReturningResult;
                            movement.status.code = Some("result_return_retry_required".into());
                            changed = true;
                        }
                    } else if movement.result_snapshot.is_none()
                        && movement.result_identity.is_none()
                    {
                        let captured = movement
                            .task_workspace
                            .as_ref()
                            .ok_or_else(|| {
                                AppError::InvalidInput(
                                    "Native Agent task workspace is unavailable.".into(),
                                )
                            })
                            .and_then(|workspace| {
                                snapshot_exact_workspace_result(
                                    &paths,
                                    &movement.status.movement_id,
                                    workspace,
                                )
                            });
                        match captured {
                            Ok((snapshot, identity)) => {
                                movement.result_snapshot = Some(snapshot);
                                movement.result_identity = Some(identity);
                                movement.status.state =
                                    NativeAgentWorkspaceMovementStateV1::ReturningResult;
                                movement.status.code = Some("result_return_retry_required".into());
                            }
                            Err(_) => {
                                movement.status.state = NativeAgentWorkspaceMovementStateV1::Failed;
                                movement.status.code =
                                    Some("native_agent_result_snapshot_recovery_failed".into());
                            }
                        }
                        changed = true;
                    } else {
                        // A partial or invalid persisted pair cannot be
                        // replaced with a new identity. Keep execution
                        // Completed and report a consequence-side failure.
                        movement.status.state = NativeAgentWorkspaceMovementStateV1::Failed;
                        movement.status.code =
                            Some("native_agent_result_snapshot_recovery_failed".into());
                        changed = true;
                    }
                } else if !matches!(
                    movement.status.state,
                    NativeAgentWorkspaceMovementStateV1::AwaitingApproval
                        | NativeAgentWorkspaceMovementStateV1::Completed
                        | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                        | NativeAgentWorkspaceMovementStateV1::Failed
                        | NativeAgentWorkspaceMovementStateV1::Cancelled
                ) {
                    let requester_has_reconciled_return = movement.source.is_some()
                        && persisted.task.state == NativeAgentTaskStateV1::Completed
                        && movement.status.state
                            == NativeAgentWorkspaceMovementStateV1::ReturningResult
                        && movement.status.code.as_deref() == Some("result_return_retry_required")
                        && movement.expected_return_digest.is_some()
                        && movement.result_identity.is_none();
                    let requester_has_capture_failure = movement.source.is_some()
                        && persisted.task.state == NativeAgentTaskStateV1::Completed
                        && movement.status.state
                            == NativeAgentWorkspaceMovementStateV1::Interrupted
                        && movement.status.code.as_deref()
                            == Some("native_agent_result_snapshot_recovery_failed");
                    // A snapshotted result remains deliverable, but never
                    // assumes that an unobserved apply or native turn happened.
                    if requester_has_reconciled_return || requester_has_capture_failure {
                        // This authenticated consequence was already durable.
                    } else if movement.result_snapshot.is_some()
                        && movement.result_identity.is_some()
                        && movement.status.state
                            == NativeAgentWorkspaceMovementStateV1::ReturningResult
                    {
                        movement.status.code = Some("result_return_retry_required".into());
                    } else {
                        movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                        movement.status.code = Some("native_agent_reconciliation_required".into());
                    }
                    changed = true;
                }
                if movement.prepared_remote.is_some()
                    && matches!(
                        persisted.task.state,
                        NativeAgentTaskStateV1::Failed | NativeAgentTaskStateV1::Cancelled
                    )
                {
                    if let Some(snapshot) = movement.result_snapshot.as_ref() {
                        delete_exact_app_owned_result(&paths, snapshot)?;
                        movement.result_snapshot = None;
                    }
                    if let Some(workspace) = movement.task_workspace.as_ref() {
                        crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
                        if workspace.exists() {
                            return invalid(
                                "Native Agent terminal task workspace cleanup is incomplete.",
                            );
                        }
                        movement.task_workspace = None;
                    }
                    movement.status.state =
                        if persisted.task.state == NativeAgentTaskStateV1::Failed {
                            NativeAgentWorkspaceMovementStateV1::Failed
                        } else {
                            NativeAgentWorkspaceMovementStateV1::Cancelled
                        };
                    movement.status.code = persisted.task.code.clone();
                    changed = true;
                }
                if movement.prepared_remote.is_some()
                    && movement.apply_completed
                    && movement.status.state == NativeAgentWorkspaceMovementStateV1::Completed
                {
                    if let Some(snapshot) = movement.result_snapshot.as_ref() {
                        delete_exact_app_owned_result(&paths, snapshot)?;
                        movement.result_snapshot = None;
                        changed = true;
                    }
                    if let Some(workspace) = movement
                        .task_workspace
                        .as_ref()
                        .or(persisted.task_workspace.as_ref())
                    {
                        crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
                        if workspace.exists() {
                            return invalid(
                                "Native Agent completed task workspace cleanup is incomplete.",
                            );
                        }
                        movement.task_workspace = None;
                        persisted.task_workspace = None;
                        changed = true;
                    }
                }
                self.workspace_movements
                    .insert(movement.status.movement_id.clone(), movement.clone());
                if movement.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled
                    && movement.status.code.as_deref() == Some("conflict_result_discard_pending")
                {
                    pending_conflict_discards.push(movement.status.movement_id.clone());
                }
            }
            self.task_workspaces.extend(
                persisted
                    .task_workspace
                    .clone()
                    .map(|workspace| (persisted.task.task_id.clone(), workspace)),
            );
            if let Some(target) = persisted.remote_target.clone() {
                self.remote_targets
                    .insert(persisted.task.task_id.clone(), target);
            }
            if let Some(digest) = persisted.task_digest.clone() {
                self.task_digests
                    .insert(persisted.task.task_id.clone(), digest);
            }
            if let Some(key) = persisted.exclusivity_key.clone() {
                self.task_exclusivity
                    .insert(persisted.task.task_id.clone(), key);
            }
            if let Some(bridge_id) = persisted.bridge_id.clone().or_else(|| {
                persisted
                    .movement
                    .as_ref()
                    .and_then(|movement| movement.bridge_id.clone())
            }) {
                self.task_bridges
                    .insert(persisted.task.task_id.clone(), bridge_id);
            }
            self.tasks
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent task store is unavailable.".into())
                })?
                .insert(persisted.task.task_id.clone(), persisted.task.clone());
            if changed {
                if unbound_local_restart {
                    crate::storage::delete_native_agent_envelope(&paths, &persisted.task.task_id)?;
                } else {
                    self.persist_envelope(
                        &persisted.task.task_id,
                        persisted.task_digest.as_deref(),
                    )?;
                }
            }
        }
        for movement_id in pending_conflict_discards {
            // The cancellation marker was durable before any retained-result
            // deletion. On restart, finish only this Host-private cleanup;
            // never contact the Agent or source workspace.
            let _ = self.finish_pending_conflict_discard(&movement_id);
        }
        Ok(())
    }

    fn task_digest(task: &str) -> String {
        blake3::hash(task.as_bytes()).to_hex().to_string()
    }

    fn persist_envelope(&self, task_id: &str, task_digest: Option<&str>) -> AppResult<()> {
        let Some(paths) = self.durable_paths.as_ref() else {
            return Ok(());
        };
        let task = self.task_status(task_id)?;
        let movement = self
            .workspace_movements
            .values()
            .find(|record| record.status.task_id == task_id)
            .cloned();
        if movement.is_none() && self.unstarted_review_tasks.contains(task_id) {
            // A direct Review is process-local; a task it never started stays
            // so. A received workspace's envelope already exists and keeps
            // owning the landed tree.
            return Ok(());
        }
        // Native requests are process-local display state: their content
        // (commands, paths, diffs) is never written to disk.
        let task = NativeAgentTaskStatusV1 {
            native_decisions: Vec::new(),
            ..task
        };
        let persisted = PersistedNativeAgentEnvelopeV1 {
            task: task.clone(),
            task_workspace: self.task_workspaces.get(task_id).cloned(),
            remote_target: self.remote_targets.get(task_id).cloned(),
            task_digest: task_digest.map(str::to_owned).or_else(|| {
                crate::storage::get_native_agent_envelope(paths, task_id)
                    .ok()
                    .flatten()
                    .and_then(|record| {
                        serde_json::from_str::<PersistedNativeAgentEnvelopeV1>(&record.record_json)
                            .ok()
                    })
                    .and_then(|record| record.task_digest)
            }),
            bridge_id: self
                .task_bridges
                .get(task_id)
                .cloned()
                .or_else(|| movement.as_ref().and_then(|value| value.bridge_id.clone())),
            movement: movement.clone(),
            exclusivity_key: self.task_exclusivity.get(task_id).cloned(),
        };
        let immutable_correlation = serde_json::to_string(&json!({
            "taskId": task_id,
            "movementId": movement.as_ref().map(|value| &value.status.movement_id),
            "targetHostRef": movement.as_ref().map(|value| &value.status.target_host_ref)
                .or_else(|| persisted.remote_target.as_ref()),
            "taskDigest": persisted.task_digest,
        }))?;
        crate::storage::save_native_agent_envelope(
            paths,
            &crate::storage::StoredNativeAgentEnvelope {
                task_id: task_id.into(),
                movement_id: movement.map(|value| value.status.movement_id),
                immutable_correlation,
                record_json: serde_json::to_string(&persisted)?,
                updated_at: crate::storage::now_ts(),
            },
        )
    }

    fn persist_task_status_after_native_turn(
        paths: &crate::storage::AppPaths,
        task_id: &str,
        status: &NativeAgentTaskStatusV1,
    ) {
        let status = NativeAgentTaskStatusV1 {
            native_decisions: Vec::new(),
            ..status.clone()
        };
        if let Ok(task) = serde_json::to_value(&status) {
            let _ =
                crate::storage::update_native_agent_observed_task_if_present(paths, task_id, &task);
        }
    }
    /// Session discovery's sources, taken under the service lock and used
    /// after the caller releases it: discovery launches bounded read-only
    /// processes. The threads Pastey's own app-servers have open are named,
    /// so discovery reports them as Pastey's, never as another client's.
    pub(crate) fn session_sources(
        &self,
    ) -> (
        sessions::ClaudeSessionSourceV1,
        sessions::CodexSessionSourceV1,
    ) {
        (
            sessions::ClaudeSessionSourceV1::installed(),
            sessions::CodexSessionSourceV1::new(
                self.codex.executable(),
                self.codex.own_thread_holds().observe(),
            ),
        )
    }

    /// The registered adapters, for observation after the caller has
    /// released the service lock.
    pub(crate) fn capability_registry(&self) -> NativeCapabilityRegistryV1 {
        NativeCapabilityRegistryV1(self.adapters.clone())
    }

    #[cfg(test)]
    pub(crate) fn capabilities(&self) -> Vec<NativeAgentCapabilityV1> {
        self.capability_registry().capabilities()
    }

    /// One existing capability fact per registered native capability.
    #[cfg(test)]
    pub(crate) fn native_capability_facts(
        &self,
    ) -> Vec<crate::peer_capabilities::HostCapabilityFact> {
        self.capability_registry().native_capability_facts()
    }

    pub(crate) fn require_capability(&self, capability_id: &str) -> AppResult<()> {
        self.adapter(capability_id)?.require_available()
    }

    /// Workspace movement is offered only by the Codex capability.
    pub(crate) fn require_codex_compatibility(&self) -> AppResult<()> {
        self.require_capability(CODEX_CAPABILITY_ID)
    }

    /// Creates the user-visible movement Review on the initiating Host. This
    /// only captures an exact safe baseline; no package or transfer exists
    /// until the requester approval below. That approval authorizes the
    /// movement, never execution: the receiving Host's executor Review does.
    pub(crate) fn propose_workspace_movement(
        &mut self,
        movement_id: &str,
        task_id: &str,
        source_workspace: &Path,
        target_host_ref: &str,
        source_object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        validate_movement_id(movement_id)?;
        validate_task_id(task_id)?;
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent workspace movement task is invalid.");
        }
        if target_host_ref.trim().is_empty() || target_host_ref.len() > 256 {
            return invalid("Native Agent movement target Host is invalid.");
        }
        let source_workspace = source_workspace.canonicalize().map_err(|_| {
            AppError::InvalidInput("Native Agent source workspace is unavailable.".into())
        })?;
        validate_workspace_transfer_fidelity(&source_workspace)?;
        let baseline = crate::safe_file_identity::capture_regular_file_set_identity(
            &source_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if let Some(existing) = self.workspace_movements.get(movement_id) {
            if existing.source.as_ref().map(|source| &source.workspace) == Some(&source_workspace)
                && existing.status.task_id == task_id
                && existing.status.target_host_ref == target_host_ref
                && existing.source.as_ref().map(|source| source.task.as_str()) == Some(task)
                && existing.source.as_ref().map(|source| source.resume) == Some(resume)
            {
                return Ok(existing.status.clone());
            }
            return invalid("Native Agent movement identity was replayed for another workspace.");
        }
        if self.task_owner(task_id)?.is_some() {
            return invalid("Native Agent movement task identity is already in use.");
        }
        let source_workspace_name = source_workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned();
        let status = NativeAgentWorkspaceMovementV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id: task_id.into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            source_workspace_name: source_workspace_name.clone(),
            target_host_ref: target_host_ref.into(),
            review_summary: format!(
                "Pastey will send \"{source_workspace_name}\" to the selected device and ask it to run Codex. Codex runs only if that device accepts; the result then returns here."
            ),
            state: NativeAgentWorkspaceMovementStateV1::AwaitingApproval,
            code: None,
        };
        self.workspace_movements.insert(
            movement_id.into(),
            WorkspaceMovementRecordV1 {
                status: status.clone(),
                source: Some(LocalMovementSourceV1 {
                    workspace: source_workspace,
                    baseline,
                    object: source_object,
                    task: task.into(),
                    resume,
                }),
                prepared_remote: None,
                task_workspace: None,
                result_snapshot: None,
                result_identity: None,
                expected_return_digest: None,
                bridge_id: None,
                apply_completed: false,
                apply_transaction: None,
            },
        );
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(
                task_id.into(),
                NativeAgentTaskStatusV1 {
                    schema_version: "pastey-native-agent-task-v1".into(),
                    task_id: task_id.into(),
                    agent_id: CODEX_CAPABILITY_ID.into(),
                    workspace_name: source_workspace_name,
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Queued,
                    result: None,
                    code: Some("workspace_movement_review".into()),
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                },
            );
        self.remote_targets
            .insert(task_id.into(), target_host_ref.into());
        self.task_digests
            .insert(task_id.into(), Self::task_digest(task));
        self.persist_envelope(task_id, Some(&Self::task_digest(task)))?;
        Ok(status)
    }

    pub(crate) fn propose_bridge_workspace_movement(
        &mut self,
        bridge_id: &str,
        movement_id: &str,
        task_id: &str,
        source_workspace: &Path,
        target_host_ref: &str,
        source_object: crate::bridge_plan_v2::ManagedObjectRevisionV2,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        self.with_bridge_task_owner(bridge_id, task_id, |service| {
            let status = service.propose_workspace_movement(
                movement_id,
                task_id,
                source_workspace,
                target_host_ref,
                source_object,
                task,
                resume,
            )?;
            if let Some(record) = service.workspace_movements.get_mut(movement_id) {
                record.bridge_id = Some(bridge_id.to_owned());
            }
            // The proposal's envelope already carries this owner, so a failed
            // write here leaves memory and disk agreeing on it.
            service.persist_envelope(task_id, None)?;
            Ok(status)
        })
    }

    /// Revalidates the approved source and frames it with the established
    /// RegularFileSet Transfer package.  The caller sends that package through
    /// the existing encrypted Room transfer; this service never transports it.
    pub(crate) fn approve_workspace_movement(
        &mut self,
        movement_id: &str,
        bridge_id: &str,
        local_host_ref: &str,
        temp_dir: &Path,
    ) -> AppResult<(
        NativeAgentWorkspacePrepareV1,
        NativeAgentWorkspaceTransferV1,
        PathBuf,
    )> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        let source_workspace = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        if source_workspace.status.state != NativeAgentWorkspaceMovementStateV1::AwaitingApproval {
            return invalid("Native Agent workspace movement is not awaiting Review approval.");
        }
        // Approval sends the movement over its owning Bridge; it never adopts
        // an unbound or another Bridge's movement.
        if source_workspace.bridge_id.as_deref() != Some(bridge_id)
            || self.task_owner(&source_workspace.status.task_id)?
                != Some(TaskOwnerV1::Bridge(bridge_id.to_owned()))
        {
            return invalid("Native Agent movement crossed its Bridge authority.");
        }
        let source_workspace = source_workspace
            .source
            .as_ref()
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Native Agent movement source is unavailable on this Host.".into(),
                )
            })?
            .workspace
            .clone();
        if self.workspace_has_authoritative_owner(&source_workspace, None, Some(movement_id))? {
            return invalid(
                "Another Native Agent execution or movement already owns this source workspace.",
            );
        }
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .expect("checked above");
        let source = record.source.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "Native Agent movement source is unavailable on this Host.".into(),
            )
        })?;
        validate_workspace_transfer_fidelity(&source.workspace)?;
        let package = crate::regular_file_set_transfer::prepare_package(
            &source.workspace,
            &source.workspace,
            &source.baseline,
            temp_dir,
        )?;
        let prior_status = record.status.clone();
        let prior_bridge_id = record.bridge_id.clone();
        record.status.state = NativeAgentWorkspaceMovementStateV1::TransferringToAgent;
        record.bridge_id = Some(bridge_id.into());
        let prepare = NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id: record.status.task_id.clone(),
            source_host_ref: local_host_ref.into(),
            target_host_ref: record.status.target_host_ref.clone(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: source.task.clone(),
            resume: source.resume,
            source_object: source.object.clone(),
            source_digest: source.baseline.digest.clone(),
            source_bytes: source.baseline.byte_count,
        };
        let metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id: record.status.task_id.clone(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Outbound,
            bridge_id: bridge_id.into(),
            source_host_ref: local_host_ref.into(),
            destination_host_ref: record.status.target_host_ref.clone(),
            object: source.object.clone(),
            content_digest: source.baseline.digest.clone(),
            logical_byte_count: source.baseline.byte_count,
        };
        let task_id = record.status.task_id.clone();
        let _ = record;
        if let Err(error) = self.persist_envelope(&task_id, None) {
            let record = self
                .workspace_movements
                .get_mut(movement_id)
                .expect("checked above");
            record.status = prior_status;
            record.bridge_id = prior_bridge_id;
            crate::regular_file_set_transfer::cleanup_package(&package);
            return Err(error);
        }
        Ok((prepare, metadata, package))
    }

    pub(crate) fn accept_workspace_prepare(
        &mut self,
        request: NativeAgentWorkspacePrepareV1,
    ) -> AppResult<()> {
        validate_workspace_prepare(&request)?;
        let task_digest = Self::task_digest(&request.task);
        if let Some(existing) = self.workspace_movements.get(&request.movement_id) {
            let prepared = existing.prepared_remote.as_ref();
            if existing.status.task_id == request.task_id
                && prepared.is_some_and(|value| {
                    value.source_host_ref == request.source_host_ref
                        && value.task == request.task
                        && value.resume == request.resume
                        && value.source_object == request.source_object
                        && value.source_digest == request.source_digest
                        && value.source_bytes == request.source_bytes
                })
            {
                return Ok(());
            }
            return invalid("Native Agent workspace movement replay does not match its task.");
        }
        if self.task_owner(&request.task_id)?.is_some() {
            return invalid("Native Agent workspace movement task identity is already in use.");
        }
        let status = NativeAgentWorkspaceMovementV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: request.movement_id.clone(),
            task_id: request.task_id.clone(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            source_workspace_name: "transferred workspace".into(),
            target_host_ref: request.target_host_ref.clone(),
            review_summary: "Approved workspace transfer is awaiting arrival.".into(),
            state: NativeAgentWorkspaceMovementStateV1::TransferringToAgent,
            code: None,
        };
        self.workspace_movements.insert(
            request.movement_id,
            WorkspaceMovementRecordV1 {
                status,
                source: None,
                prepared_remote: Some(PreparedRemoteWorkspaceV1 {
                    source_host_ref: request.source_host_ref,
                    task: request.task,
                    resume: request.resume,
                    source_object: request.source_object,
                    source_digest: request.source_digest,
                    source_bytes: request.source_bytes,
                }),
                task_workspace: None,
                result_snapshot: None,
                result_identity: None,
                expected_return_digest: None,
                bridge_id: None,
                apply_completed: false,
                apply_transaction: None,
            },
        );
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(
                request.task_id.clone(),
                NativeAgentTaskStatusV1 {
                    schema_version: "pastey-native-agent-task-v1".into(),
                    task_id: request.task_id.clone(),
                    agent_id: CODEX_CAPABILITY_ID.into(),
                    workspace_name: "transferred workspace".into(),
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Queued,
                    result: None,
                    code: Some("workspace_transfer_pending".into()),
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                },
            );
        self.task_digests
            .insert(request.task_id.clone(), task_digest.clone());
        self.persist_envelope(&request.task_id, Some(&task_digest))?;
        Ok(())
    }

    /// Accepts preparation authority from the exact authenticated current
    /// peer session `binding`, whose HostRef must be the claimed source. The
    /// binding becomes the principal of the executor Review that the landed
    /// workspace waits for: requester approval authorizes the movement, never
    /// execution on this Host.
    pub(crate) fn accept_bridge_workspace_prepare(
        &mut self,
        binding: &HostSessionBinding,
        request: NativeAgentWorkspacePrepareV1,
    ) -> AppResult<()> {
        let movement_id = request.movement_id.clone();
        self.accept_bridge_workspace_prepare_from(
            &binding.bridge_id,
            binding.peer_host_ref.as_str(),
            request,
        )?;
        self.movement_bindings.insert(movement_id, binding.clone());
        Ok(())
    }

    /// Preparation authority from the authenticated peer whose durable
    /// HostRef is `authenticated_source_host_ref`. The claimed source must be
    /// that exact Host before anything is prepared.
    fn accept_bridge_workspace_prepare_from(
        &mut self,
        bridge_id: &str,
        authenticated_source_host_ref: &str,
        request: NativeAgentWorkspacePrepareV1,
    ) -> AppResult<()> {
        if request.source_host_ref != authenticated_source_host_ref {
            return invalid("Native Agent workspace source is not the authenticated peer Host.");
        }
        let task_id = request.task_id.clone();
        let movement_id = request.movement_id.clone();
        self.with_bridge_task_owner(bridge_id, &task_id, |service| {
            service.accept_workspace_prepare(request)?;
            if let Some(record) = service.workspace_movements.get_mut(&movement_id) {
                record.bridge_id = Some(bridge_id.to_owned());
            }
            service.persist_envelope(&task_id, None)
        })
    }

    pub(crate) fn validate_workspace_transfer(
        &self,
        metadata: &NativeAgentWorkspaceTransferV1,
        local_host_ref: &str,
    ) -> AppResult<()> {
        validate_workspace_transfer(metadata)?;
        if metadata.destination_host_ref != local_host_ref {
            return invalid("Native Agent workspace Transfer targets another Host.");
        }
        let record = self
            .workspace_movements
            .get(&metadata.movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        if record.status.task_id != metadata.task_id {
            return invalid("Native Agent workspace Transfer crossed its task binding.");
        }
        if record.bridge_id.as_deref() != Some(metadata.bridge_id.as_str())
            || self.revoked_bridges.contains(&metadata.bridge_id)
        {
            return invalid("Native Agent workspace Transfer crossed its Bridge authority.");
        }
        match metadata.phase {
            NativeAgentWorkspaceTransferPhaseV1::Outbound => {
                let prepared = record.prepared_remote.as_ref().ok_or_else(|| {
                    AppError::InvalidInput(
                        "Native Agent workspace was not prepared on this Host.".into(),
                    )
                })?;
                if prepared.source_host_ref != metadata.source_host_ref
                    || prepared.source_object != metadata.object
                    || prepared.source_digest != metadata.content_digest
                    || prepared.source_bytes != metadata.logical_byte_count
                    || record.status.state
                        != NativeAgentWorkspaceMovementStateV1::TransferringToAgent
                {
                    return invalid(
                        "Native Agent outbound workspace Transfer does not match Review.",
                    );
                }
            }
            NativeAgentWorkspaceTransferPhaseV1::Return => {
                let source = record.source.as_ref().ok_or_else(|| {
                    AppError::InvalidInput(
                        "Native Agent result return arrived at the wrong Host.".into(),
                    )
                })?;
                let returned = record.result_identity.as_ref();
                let duplicate_completed = record.apply_completed
                    && record.status.state == NativeAgentWorkspaceMovementStateV1::Completed
                    && returned.is_some_and(|identity| {
                        identity.digest == metadata.content_digest
                            && identity.byte_count == metadata.logical_byte_count
                    });
                let conflict_retention_pending =
                    record.result_identity.as_ref().is_some_and(|identity| {
                        identity.digest == metadata.content_digest
                            && identity.byte_count == metadata.logical_byte_count
                            && matches!(
                                record.status.state,
                                NativeAgentWorkspaceMovementStateV1::ApplyingResult
                                    | NativeAgentWorkspaceMovementStateV1::Interrupted
                            )
                            && record.status.code.as_deref().is_some_and(|code| {
                                code == "conflict_result_retention_pending"
                                    || code == "conflict_result_retention_required"
                                    || code == "conflict_result_retention_failed"
                            })
                    });
                let apply_repair_authorized =
                    record.status.state == NativeAgentWorkspaceMovementStateV1::Interrupted
                        && record.status.code.as_deref() == Some("result_apply_interrupted")
                        && returned.is_some_and(|identity| {
                            identity.digest == metadata.content_digest
                                && identity.byte_count == metadata.logical_byte_count
                                && record.apply_transaction.as_ref().is_none_or(|transaction| {
                                    transaction.result_identity == *identity
                                })
                        });
                let duplicate_conflict = record.status.state
                    == NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                    && record.result_identity.as_ref().is_some_and(|identity| {
                        identity.digest == metadata.content_digest
                            && identity.byte_count == metadata.logical_byte_count
                    });
                if metadata.destination_host_ref != local_host_ref
                    || metadata.source_host_ref != record.status.target_host_ref
                    || record
                        .expected_return_digest
                        .as_ref()
                        .is_some_and(|digest| digest != &metadata.content_digest)
                    || (!duplicate_completed
                        && !duplicate_conflict
                        && !conflict_retention_pending
                        && !apply_repair_authorized
                        && record.status.state
                            != NativeAgentWorkspaceMovementStateV1::ReturningResult)
                    || source.workspace.as_os_str().is_empty()
                {
                    return invalid(
                        "Native Agent result Transfer does not match its approved movement.",
                    );
                }
            }
        }
        Ok(())
    }

    fn return_is_already_applied(&self, metadata: &NativeAgentWorkspaceTransferV1) -> bool {
        self.workspace_movements
            .get(&metadata.movement_id)
            .filter(|record| record.status.task_id == metadata.task_id)
            .is_some_and(|record| {
                record.apply_completed
                    && record.status.state == NativeAgentWorkspaceMovementStateV1::Completed
                    && record.result_identity.as_ref().is_some_and(|identity| {
                        identity.digest == metadata.content_digest
                            && identity.byte_count == metadata.logical_byte_count
                    })
            })
    }

    fn return_is_already_conflicted(
        &self,
        metadata: &NativeAgentWorkspaceTransferV1,
        paths: &crate::storage::AppPaths,
    ) -> bool {
        self.workspace_movements
            .get(&metadata.movement_id)
            .filter(|record| record.status.task_id == metadata.task_id)
            .is_some_and(|record| {
                record.status.state == NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                    && record.result_identity.as_ref().is_some_and(|identity| {
                        identity.digest == metadata.content_digest
                            && identity.byte_count == metadata.logical_byte_count
                    })
                    && crate::storage::get_native_agent_conflict(paths, &metadata.movement_id)
                        .ok()
                        .flatten()
                        .is_some_and(|conflict| {
                            conflict.task_id == metadata.task_id
                                && conflict.result_digest == metadata.content_digest
                                && conflict.result_byte_count == metadata.logical_byte_count
                                && validate_retained_conflict(paths, &conflict).is_ok()
                        })
            })
    }

    /// Binds the received file-set to the pre-authorized task workspace and
    /// submits the task to executor admission. The native Agent will see only
    /// that ordinary local directory, and only after this Host's Review.
    pub(crate) fn start_received_workspace_task(
        &mut self,
        movement_id: &str,
        task_workspace: &Path,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_received_workspace_task_with_executable(
            Path::new("codex"),
            movement_id,
            task_workspace,
        )
    }

    /// The landing of an approved outbound workspace. The requester's
    /// movement approval authorizes moving the workspace, never execution on
    /// this Host: once the landed tree is durably owned, the task goes
    /// through the same admission as a direct remote invocation, as the peer
    /// session that prepared the movement. It stays Queued, unprepared and
    /// unstarted, until this Host's user accepts its Review.
    fn start_received_workspace_task_with_executable(
        &mut self,
        executable: &Path,
        movement_id: &str,
        task_workspace: &Path,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let (task_id, task, resume, landed) = {
            let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            let prepared = record.prepared_remote.as_ref().ok_or_else(|| {
                AppError::InvalidInput(
                    "Native Agent workspace was not prepared on this Host.".into(),
                )
            })?;
            (
                record.status.task_id.clone(),
                prepared.task.clone(),
                prepared.resume,
                record.task_workspace.is_some(),
            )
        };
        // A cancellation may have reached this Host after workspace prepare
        // but before the authenticated outbound landing. Its terminal outer
        // authority prevents this landing from starting a native turn.
        if self
            .workspace_movements
            .get(movement_id)
            .is_some_and(|record| {
                record.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled
            })
        {
            return self.task_status(&task_id);
        }
        // One landing per movement: a duplicate never replaces the owned tree
        // or creates a second Review.
        if landed {
            return invalid("Native Agent workspace has already landed for this movement.");
        }
        let binding = self
            .movement_bindings
            .get(movement_id)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Native Agent workspace movement has no authenticated session.".into(),
                )
            })?;
        if !resume && self.codex.has_session(task_workspace) {
            return invalid(
                "Native Agent workspace already has a session; resumption is required.",
            );
        }
        let canonical_workspace = task_workspace.canonicalize().map_err(|_| {
            AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
        })?;
        self.codex.use_executable(executable);
        // Assembled only; no adapter reads it before admission.
        let input = capability::codex::codex_input(&canonical_workspace, &task, true)?;
        // Commit ownership of the materialized tree while the task is still
        // queued. If this write fails, no native turn has started and landing
        // cleanup may safely remove the tree.
        self.workspace_movements
            .get_mut(movement_id)
            .expect("checked above")
            .task_workspace = Some(canonical_workspace);
        if let Err(error) = self.persist_envelope(&task_id, None) {
            self.workspace_movements
                .get_mut(movement_id)
                .expect("checked above")
                .task_workspace = None;
            return Err(error);
        }
        let admitted = self.admit_invocation(
            NativeInvocationPrincipalV1::Remote(&binding),
            CODEX_CAPABILITY_ID,
            &task_id,
            &input,
            NativeInvocationContinuationV1::ReceivedWorkspace {
                movement_id: movement_id.into(),
            },
            crate::storage::now_ts(),
        );
        if admitted.is_err() {
            // Nothing started; give the tree back to landing cleanup.
            if let Some(record) = self.workspace_movements.get_mut(movement_id) {
                record.task_workspace = None;
            }
            let _ = self.persist_envelope(&task_id, None);
        }
        admitted
    }

    /// Starts an admitted received-workspace task: the existing movement
    /// start, unchanged. On failure nothing has started and the task is
    /// Queued again; the caller decides what happens to the landed tree.
    fn start_admitted_received_workspace(
        &mut self,
        movement_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let canonical_workspace = self
            .workspace_movements
            .get(movement_id)
            .and_then(|record| record.task_workspace.clone())
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
            })?;
        // The queued entry is only the pre-transfer envelope, not proof of a
        // native turn. Replace it exactly once after the tree is durable.
        let queued = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .remove(task_id);
        let was_unstarted_review = self.unstarted_review_tasks.remove(task_id);
        let had_session = self.codex.has_session(&canonical_workspace);
        // A landed workspace is fresh for each movement: its conversation is
        // the movement's own and is not kept past it.
        let started = self.start_invocation_in_movement(
            CODEX_CAPABILITY_ID,
            task_id,
            input,
            Some(movement_id),
            None,
        );
        let status = match started {
            Ok(status) => status,
            Err(error) => {
                // No turn was spawned on this error path. Close an app-server
                // created for this landing before its tree may be cleaned.
                if !had_session {
                    self.codex.shutdown_session(&canonical_workspace);
                }
                self.task_workspaces.remove(task_id);
                if let Some(queued) = queued {
                    self.tasks
                        .lock()
                        .map_err(|_| {
                            AppError::InvalidInput("Native Agent task store is unavailable.".into())
                        })?
                        .insert(task_id.to_owned(), queued);
                }
                if was_unstarted_review {
                    self.unstarted_review_tasks.insert(task_id.to_owned());
                }
                let _ = self.persist_envelope(task_id, None);
                return Err(error);
            }
        };
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        record.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
        let task_id = record.status.task_id.clone();
        let _ = record;
        // The turn is already running. A failed projection write cannot make
        // the caller delete its live cwd; the prior durable envelope still
        // owns the exact task workspace for restart reconciliation.
        #[cfg(test)]
        let persisted = if std::mem::take(&mut self.fail_post_start_persist_once) {
            invalid("simulated post-start Native Agent envelope write failure")
        } else {
            self.persist_envelope(&task_id, None)
        };
        #[cfg(not(test))]
        let persisted = self.persist_envelope(&task_id, None);
        let _ = persisted;
        Ok(status)
    }

    /// Accept's movement-specific revalidation: the landed workspace and the
    /// input held for Review still belong to exactly this movement and task.
    fn revalidate_received_workspace(
        &self,
        pending: &PendingNativeInvocationV1,
        movement_id: &str,
    ) -> AppResult<()> {
        let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        let prepared = record.prepared_remote.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace was not prepared on this Host.".into())
        })?;
        let workspace = record.task_workspace.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
        })?;
        if record.status.task_id != pending.task_id
            || record.bridge_id.as_deref() != Some(pending.binding.bridge_id.as_str())
            || record.status.state != NativeAgentWorkspaceMovementStateV1::TransferringToAgent
            || pending.capability_id != CODEX_CAPABILITY_ID
            || !workspace.is_dir()
            || capability::codex::codex_input(workspace, &prepared.task, true)? != pending.input
        {
            return invalid("Native Agent workspace no longer matches its Review.");
        }
        Ok(())
    }

    pub(crate) fn movement_status(
        &self,
        movement_id: &str,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        self.workspace_movements
            .get(movement_id)
            .map(|record| record.status.clone())
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })
    }

    pub(crate) fn completed_task_workspace_for_return(
        &mut self,
        movement_id: &str,
    ) -> AppResult<(String, PathBuf, String)> {
        let (task_id, source_host_ref, workspace) = {
            let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            let prepared = record.prepared_remote.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace is not owned by this Host.".into())
            })?;
            (
                record.status.task_id.clone(),
                prepared.source_host_ref.clone(),
                record.task_workspace.clone().ok_or_else(|| {
                    AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
                })?,
            )
        };
        if self.task_status(&task_id)?.state != NativeAgentTaskStateV1::Completed {
            return invalid("Native Agent result is not available for return.");
        }
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .expect("checked above");
        record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        let task_id = record.status.task_id.clone();
        let _ = record;
        self.persist_envelope(&task_id, None)?;
        Ok((task_id, workspace, source_host_ref))
    }

    /// Captures the exact post-Agent file set before attempting a return. The
    /// durable snapshot, rather than a live Agent workspace, is the only thing
    /// eligible for retry after an acknowledgement or transport loss.
    fn captured_result_snapshot_for_return(
        &mut self,
        movement_id: &str,
    ) -> AppResult<(
        String,
        PathBuf,
        String,
        crate::safe_file_identity::RegularFileSetIdentity,
    )> {
        if let Some(record) = self.workspace_movements.get(movement_id) {
            if record.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled {
                return invalid("Native Agent workspace movement was cancelled.");
            }
            if let (Some(snapshot), Some(identity), Some(prepared)) = (
                record.result_snapshot.as_ref(),
                record.result_identity.as_ref(),
                record.prepared_remote.as_ref(),
            ) {
                let paths = self.durable_paths.as_ref().ok_or_else(|| {
                    AppError::InvalidInput("Native Agent result persistence is unavailable.".into())
                })?;
                validate_exact_app_owned_result(paths, snapshot, identity)?;
                return Ok((
                    record.status.task_id.clone(),
                    snapshot.clone(),
                    prepared.source_host_ref.clone(),
                    identity.clone(),
                ));
            }
        }
        let (_completed_task_id, workspace, source_host_ref) =
            self.completed_task_workspace_for_return(movement_id)?;
        let (snapshot, identity) = {
            let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            match (&record.result_snapshot, &record.result_identity) {
                (Some(snapshot), Some(identity)) => (snapshot.clone(), identity.clone()),
                _ => {
                    let paths = self.durable_paths.as_ref().ok_or_else(|| {
                        AppError::InvalidInput(
                            "Native Agent result persistence is unavailable.".into(),
                        )
                    })?;
                    snapshot_exact_workspace_result(paths, movement_id, &workspace)?
                }
            }
        };
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .expect("checked above");
        record.result_snapshot = Some(snapshot.clone());
        record.result_identity = Some(identity.clone());
        let task_id = record.status.task_id.clone();
        let _ = record;
        self.persist_envelope(&task_id, None)?;
        Ok((task_id, snapshot, source_host_ref, identity))
    }

    pub(crate) fn mark_result_return_pending(&mut self, movement_id: &str) {
        let Some(task_id) = self
            .workspace_movements
            .get(movement_id)
            .map(|record| record.status.task_id.clone())
        else {
            return;
        };
        let completed_task = self
            .tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(&task_id).cloned())
            .is_some_and(|task| task.state == NativeAgentTaskStateV1::Completed);
        let exact_snapshot_is_valid = self.durable_paths.as_ref().is_some_and(|paths| {
            self.workspace_movements
                .get(movement_id)
                .and_then(|record| {
                    record
                        .result_snapshot
                        .as_ref()
                        .zip(record.result_identity.as_ref())
                })
                .is_some_and(|(snapshot, identity)| {
                    validate_exact_app_owned_result(paths, snapshot, identity).is_ok()
                })
        });
        let Some(record) = self.workspace_movements.get_mut(movement_id) else {
            return;
        };
        if matches!(
            record.status.state,
            NativeAgentWorkspaceMovementStateV1::Completed
                | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                | NativeAgentWorkspaceMovementStateV1::Cancelled
        ) {
            return;
        }
        if exact_snapshot_is_valid {
            record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
            record.status.code = Some("result_return_retry_required".into());
        } else if completed_task
            && record.prepared_remote.is_some()
            && record.apply_transaction.is_none()
            && !record.apply_completed
        {
            record.status.state = NativeAgentWorkspaceMovementStateV1::Failed;
            record.status.code = Some("native_agent_result_snapshot_recovery_failed".into());
        } else {
            return;
        }
        let task_id = record.status.task_id.clone();
        let _ = record;
        let _ = self.persist_envelope(&task_id, None);
    }

    pub(crate) fn authorize_result_return_retry(
        &mut self,
        bridge_id: &str,
        request: &NativeAgentRetryResultReturnV1,
        authenticated_source_host_ref: &str,
        local_host_ref: &str,
    ) -> AppResult<()> {
        validate_retry_result_return(request)?;
        if request.target_host_ref != local_host_ref
            || self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(&request.task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent result Return retry crossed its Bridge authority.");
        }
        let record = self
            .workspace_movements
            .get(&request.movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        let prepared = record.prepared_remote.as_ref().ok_or_else(|| {
            AppError::InvalidInput(
                "Native Agent result Return retry reached the wrong Host.".into(),
            )
        })?;
        let (Some(snapshot), Some(identity)) = (
            record.result_snapshot.as_ref(),
            record.result_identity.as_ref(),
        ) else {
            return invalid("Native Agent exact durable result is unavailable for retry.");
        };
        if record.status.task_id != request.task_id
            || record.status.state != NativeAgentWorkspaceMovementStateV1::ReturningResult
            || !matches!(
                record.status.code.as_deref(),
                None | Some("result_return_retry_required")
            )
            || prepared.source_host_ref != authenticated_source_host_ref
            || identity.digest.is_empty()
        {
            return invalid(
                "Native Agent result Return retry does not match its durable movement.",
            );
        }
        let paths = self.durable_paths.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent result persistence is unavailable.".into())
        })?;
        validate_exact_app_owned_result(paths, snapshot, identity)
    }

    pub(crate) fn authorize_source_apply_retry(
        &self,
        bridge_id: &str,
        movement_id: &str,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        if record.bridge_id.as_deref() != Some(bridge_id)
            || self
                .task_bridges
                .get(&record.status.task_id)
                .map(String::as_str)
                != Some(bridge_id)
            || record.source.is_none()
            || record.status.state != NativeAgentWorkspaceMovementStateV1::Interrupted
            || !matches!(
                record.status.code.as_deref(),
                Some("result_apply_interrupted")
                    | Some("conflict_result_retention_required")
                    | Some("conflict_result_retention_failed")
            )
            || record.result_identity.is_none()
            || record
                .apply_transaction
                .as_ref()
                .zip(record.result_identity.as_ref())
                .is_some_and(|(transaction, identity)| transaction.result_identity != *identity)
        {
            return invalid("Native Agent apply retry has no exact durable result to recover.");
        }
        let task = self.task_status(&record.status.task_id)?;
        if matches!(
            task.state,
            NativeAgentTaskStateV1::Failed | NativeAgentTaskStateV1::Cancelled
        ) || self
            .remote_targets
            .get(&record.status.task_id)
            .map(String::as_str)
            != Some(record.status.target_host_ref.as_str())
        {
            return invalid("Native Agent apply retry crossed its executing Host correlation.");
        }
        Ok(record.status.clone())
    }

    pub(crate) fn authorize_source_pending_return_retry(
        &self,
        bridge_id: &str,
        movement_id: &str,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        if self.revoked_bridges.contains(bridge_id)
            || record.bridge_id.as_deref() != Some(bridge_id)
            || self
                .task_bridges
                .get(&record.status.task_id)
                .map(String::as_str)
                != Some(bridge_id)
            || record.source.is_none()
            || record.prepared_remote.is_some()
            || record.status.state != NativeAgentWorkspaceMovementStateV1::ReturningResult
            || record.status.code.as_deref() != Some("result_return_retry_required")
            || record.expected_return_digest.is_none()
            || record.result_identity.is_some()
            || record.apply_transaction.is_some()
            || record.apply_completed
            || self.task_status(&record.status.task_id)?.state != NativeAgentTaskStateV1::Completed
            || self
                .remote_targets
                .get(&record.status.task_id)
                .map(String::as_str)
                != Some(record.status.target_host_ref.as_str())
        {
            return invalid("Native Agent pending Return has no exact reconciled result.");
        }
        Ok(record.status.clone())
    }

    fn validate_local_result_return_retry(
        &mut self,
        bridge_id: &str,
        movement_id: &str,
    ) -> AppResult<()> {
        let task_id = {
            let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            if self.revoked_bridges.contains(bridge_id)
                || self
                    .task_bridges
                    .get(&record.status.task_id)
                    .map(String::as_str)
                    != Some(bridge_id)
                || record.prepared_remote.is_none()
                || record.status.state != NativeAgentWorkspaceMovementStateV1::ReturningResult
                || !matches!(
                    record.status.code.as_deref(),
                    None | Some("result_return_retry_required")
                )
                || record.result_snapshot.is_none()
                || record.result_identity.is_none()
            {
                return invalid("Native Agent exact result Return is not pending retry.");
            }
            record.status.task_id.clone()
        };
        if self.task_status(&task_id)?.state != NativeAgentTaskStateV1::Completed {
            return invalid("Native Agent result Return retry has no completed task result.");
        }
        let (_, _, _, _) = self.captured_result_snapshot_for_return(movement_id)?;
        Ok(())
    }

    fn complete_workspace_result_return(&mut self, movement_id: &str) -> AppResult<()> {
        let (task_id, snapshot, task_workspace) = {
            let record = self
                .workspace_movements
                .get_mut(movement_id)
                .ok_or_else(|| {
                    AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
                })?;
            if record.status.state == NativeAgentWorkspaceMovementStateV1::Completed
                && record.apply_completed
            {
                (
                    record.status.task_id.clone(),
                    record.result_snapshot.clone(),
                    record.task_workspace.clone(),
                )
            } else {
                if record.prepared_remote.is_none()
                    || record.result_snapshot.is_none()
                    || record.result_identity.is_none()
                {
                    return invalid("Native Agent durable Return result is unavailable to commit.");
                }
                record.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
                record.status.code = None;
                record.apply_completed = true;
                (
                    record.status.task_id.clone(),
                    record.result_snapshot.clone(),
                    record.task_workspace.clone(),
                )
            }
        };
        self.persist_envelope(&task_id, None)?;

        let mut cleanup_complete = true;
        if let Some(snapshot) = snapshot.as_ref() {
            if let Some(paths) = self.durable_paths.as_ref() {
                cleanup_complete &= delete_exact_app_owned_result(paths, snapshot).is_ok();
            } else {
                cleanup_complete = false;
            }
        }
        if let Some(workspace) = task_workspace.as_ref() {
            crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
            cleanup_complete &= !workspace.exists();
        }
        if cleanup_complete {
            if let Some(record) = self.workspace_movements.get_mut(movement_id) {
                record.result_snapshot = None;
                record.task_workspace = None;
            }
            self.task_workspaces.remove(&task_id);
            self.persist_envelope(&task_id, None)?;
        }
        Ok(())
    }

    fn cleanup_terminal_remote_workspace(&mut self, movement_id: &str) -> AppResult<bool> {
        let (task_id, task_state, workspace, snapshot, prepared_remote) = {
            let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            let task = self.task_status(&record.status.task_id)?;
            (
                record.status.task_id.clone(),
                task.state,
                record.task_workspace.clone(),
                record.result_snapshot.clone(),
                record.prepared_remote.is_some(),
            )
        };
        if !prepared_remote
            || !matches!(
                task_state,
                NativeAgentTaskStateV1::Failed | NativeAgentTaskStateV1::Cancelled
            )
        {
            return Ok(false);
        }
        if let Some(workspace) = workspace.as_ref() {
            let active = self
                .active_exclusivity
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent session store is unavailable.".into())
                })?
                .contains_key(&capability::codex::workspace_exclusivity_key(workspace));
            if active {
                return Ok(false);
            }
        }
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .expect("movement was checked above");
        record.status.state = if task_state == NativeAgentTaskStateV1::Failed {
            NativeAgentWorkspaceMovementStateV1::Failed
        } else {
            NativeAgentWorkspaceMovementStateV1::Cancelled
        };
        let _ = record;
        self.persist_envelope(&task_id, None)?;

        if let Some(workspace) = workspace.as_ref() {
            self.codex.shutdown_session(workspace);
            crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
        }
        let mut complete = workspace.as_ref().is_none_or(|path| !path.exists());
        if let Some(snapshot) = snapshot.as_ref() {
            complete &= self
                .durable_paths
                .as_ref()
                .is_some_and(|paths| delete_exact_app_owned_result(paths, snapshot).is_ok());
        }
        if complete {
            if let Some(record) = self.workspace_movements.get_mut(movement_id) {
                record.task_workspace = None;
                record.result_snapshot = None;
            }
            self.task_workspaces.remove(&task_id);
            self.persist_envelope(&task_id, None)?;
        }
        Ok(complete)
    }

    pub(crate) fn reconciliation_fact(
        &self,
        bridge_id: &str,
        task_id: &str,
        movement_id: Option<&str>,
        executing_host_ref: &str,
        authenticated_source_host_ref: &str,
    ) -> AppResult<NativeAgentReconciliationV1> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent reconciliation crossed its Bridge authority.");
        }
        let task = self.task_status(task_id)?;
        let movement = match movement_id {
            Some(id) => Some(self.workspace_movements.get(id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent movement is unavailable.".into())
            })?),
            None => None,
        };
        if movement.is_some_and(|value| {
            value.status.task_id != task_id
                || value.bridge_id.as_deref() != Some(bridge_id)
                || value.prepared_remote.as_ref().is_some_and(|prepared| {
                    prepared.source_host_ref != authenticated_source_host_ref
                })
        }) {
            return invalid("Native Agent reconciliation crossed task correlation.");
        }
        Ok(NativeAgentReconciliationV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: task_id.into(),
            movement_id: movement_id.map(str::to_owned),
            executing_host_ref: executing_host_ref.into(),
            task_state: task.state.clone(),
            movement_state: movement.map(|value| value.status.state.clone()),
            result_digest: movement.and_then(|value| {
                if task.state != NativeAgentTaskStateV1::Completed
                    || value.prepared_remote.is_none()
                    || value.status.state != NativeAgentWorkspaceMovementStateV1::ReturningResult
                {
                    return None;
                }
                let (snapshot, identity, paths) = value
                    .result_snapshot
                    .as_ref()
                    .zip(value.result_identity.as_ref())
                    .zip(self.durable_paths.as_ref())
                    .map(|((snapshot, identity), paths)| (snapshot, identity, paths))?;
                validate_exact_app_owned_result(paths, snapshot, identity)
                    .ok()
                    .map(|()| identity.digest.clone())
            }),
            apply_completed: movement.is_some_and(|value| value.apply_completed),
            code: movement
                .and_then(|value| value.status.code.clone())
                .or(task.code),
            task_output: task.output,
        })
    }

    pub(crate) fn record_remote_reconciliation(
        &mut self,
        fact: NativeAgentReconciliationV1,
    ) -> AppResult<()> {
        validate_reconciliation(&fact)?;
        if self.remote_targets.get(&fact.task_id).map(String::as_str)
            != Some(fact.executing_host_ref.as_str())
        {
            return invalid("Native Agent reconciliation crossed its selected Host binding.");
        }
        if let Some(movement_id) = fact.movement_id.as_ref() {
            let movement = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent movement is unavailable.".into())
            })?;
            if movement.status.task_id != fact.task_id {
                return invalid("Native Agent reconciliation crossed movement correlation.");
            }
            if movement.status.target_host_ref != fact.executing_host_ref {
                return invalid("Native Agent reconciliation crossed movement Host correlation.");
            }
            if movement
                .expected_return_digest
                .as_ref()
                .is_some_and(|digest| {
                    fact.result_digest
                        .as_ref()
                        .is_some_and(|next| next != digest)
                })
            {
                return invalid("Native Agent reconciliation changed its exact Return digest.");
            }
        }
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks.get_mut(&fact.task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        let cancellation_won = task.state == NativeAgentTaskStateV1::Cancelled;
        if !cancellation_won {
            let next = NativeAgentTaskStatusV1 {
                state: fact.task_state.clone(),
                code: if fact.movement_id.is_some()
                    && fact.task_state == NativeAgentTaskStateV1::Completed
                {
                    None
                } else if fact.movement_id.is_some() {
                    task.code.clone()
                } else {
                    fact.code.clone()
                },
                output: fact.task_output.clone().or_else(|| task.output.clone()),
                ..task.clone()
            };
            if task_accepts_remote_fact(task, &next) {
                *task = next;
            }
        }
        let task_completed = task.state == NativeAgentTaskStateV1::Completed;
        drop(tasks);
        if cancellation_won {
            let code = self
                .task_status(&fact.task_id)?
                .code
                .unwrap_or_else(|| "native_agent_cancelled".into());
            self.cancel_matching_workspace_movement(&fact.task_id, &code);
        } else {
            if let Some(movement_id) = fact.movement_id.as_ref() {
                if let Some(movement) = self.workspace_movements.get_mut(movement_id) {
                    let requester_uncertain = movement.status.state
                        == NativeAgentWorkspaceMovementStateV1::Interrupted
                        && movement.status.code.as_deref()
                            == Some("native_agent_reconciliation_required");
                    let exact_apply_commit = task_completed
                        && fact.apply_completed
                        && fact.movement_state
                            == Some(NativeAgentWorkspaceMovementStateV1::Completed)
                        && fact.result_digest.as_deref().is_some_and(|digest| {
                            movement
                                .result_identity
                                .as_ref()
                                .is_some_and(|identity| identity.digest == digest)
                        });
                    let exact_unreceived_return = movement.source.is_some()
                        && movement.prepared_remote.is_none()
                        && !movement.apply_completed
                        && movement.apply_transaction.is_none()
                        && movement.result_identity.is_none()
                        && task_completed
                        && fact.task_state == NativeAgentTaskStateV1::Completed
                        && fact.movement_state
                            == Some(NativeAgentWorkspaceMovementStateV1::ReturningResult)
                        && fact.result_digest.is_some()
                        && !cancellation_won;
                    let exact_capture_failure = movement.source.is_some()
                        && movement.prepared_remote.is_none()
                        && !movement.apply_completed
                        && movement.apply_transaction.is_none()
                        && movement.result_identity.is_none()
                        && movement.expected_return_digest.is_none()
                        && task_completed
                        && fact.task_state == NativeAgentTaskStateV1::Completed
                        && fact.movement_state == Some(NativeAgentWorkspaceMovementStateV1::Failed)
                        && fact.result_digest.is_none()
                        && fact.code.as_deref()
                            == Some("native_agent_result_snapshot_recovery_failed");
                    if requester_uncertain && exact_apply_commit {
                        let recovered = movement.apply_transaction.is_none()
                            || recover_workspace_apply_record(movement_id, movement).is_ok();
                        let source_matches_result = movement
                            .source
                            .as_ref()
                            .zip(movement.result_identity.as_ref())
                            .is_some_and(|(source, result)| {
                                crate::safe_file_identity::capture_regular_file_set_identity(
                                    &source.workspace,
                                    crate::storage::MAX_FILE_SIZE_BYTES,
                                )
                                .is_ok_and(|current| same_logical_file_set(&current, result))
                            });
                        if recovered && source_matches_result {
                            movement.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
                            movement.status.code = None;
                            movement.apply_completed = true;
                            movement.apply_transaction = None;
                        }
                    } else if (requester_uncertain
                        || movement.status.state
                            == NativeAgentWorkspaceMovementStateV1::ReturningResult)
                        && exact_unreceived_return
                    {
                        movement.expected_return_digest = fact.result_digest;
                        movement.status.state =
                            NativeAgentWorkspaceMovementStateV1::ReturningResult;
                        movement.status.code = Some("result_return_retry_required".into());
                    } else if (requester_uncertain
                        || movement.status.state
                            == NativeAgentWorkspaceMovementStateV1::ReturningResult)
                        && exact_capture_failure
                    {
                        movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                        movement.status.code =
                            Some("native_agent_result_snapshot_recovery_failed".into());
                    } else if let Some(state) = fact.movement_state {
                        if movement_accepts_remote_execution_fact(movement, &state) {
                            movement.status.state = state;
                            movement.status.code = fact.code;
                        }
                    }
                }
            }
        }
        self.persist_envelope(&fact.task_id, None)
    }

    pub(crate) fn record_bridge_remote_reconciliation(
        &mut self,
        bridge_id: &str,
        fact: NativeAgentReconciliationV1,
    ) -> AppResult<()> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(&fact.task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent reconciliation crossed its Bridge authority.");
        }
        self.record_remote_reconciliation(fact)
    }

    pub(crate) fn interrupt_workspace_movement(&mut self, movement_id: &str, code: &str) {
        if let Some(record) = self.workspace_movements.get_mut(movement_id) {
            if !matches!(
                record.status.state,
                NativeAgentWorkspaceMovementStateV1::Completed
                    | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
            ) {
                record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                record.status.code = Some(code.into());
                let task_id = record.status.task_id.clone();
                let _ = record;
                let _ = self.persist_envelope(&task_id, None);
            }
        }
    }

    pub(crate) fn apply_received_workspace_return(
        &mut self,
        movement_id: &str,
        returned_workspace: &Path,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let record = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        if record.apply_completed
            || record.status.state == NativeAgentWorkspaceMovementStateV1::Completed
        {
            return Ok(record.status.clone());
        }
        if record.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled {
            return invalid("Native Agent workspace movement was cancelled.");
        }
        let source = record.source.clone().ok_or_else(|| {
            AppError::InvalidInput("Native Agent result return arrived at the wrong Host.".into())
        })?;
        validate_workspace_transfer_fidelity(returned_workspace)?;
        let returned = crate::safe_file_identity::capture_regular_file_set_identity(
            returned_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if record
            .expected_return_digest
            .as_ref()
            .is_some_and(|digest| digest != &returned.digest)
        {
            return invalid("Native Agent Return differs from its reconciled exact result.");
        }
        let current = crate::safe_file_identity::capture_regular_file_set_identity(
            &source.workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if current != source.baseline {
            if record
                .result_identity
                .as_ref()
                .is_some_and(|existing| !same_logical_file_set(existing, &returned))
            {
                return invalid("Native Agent conflict Return does not match its exact result.");
            }
            // Persist the exact returned identity before retention. The
            // terminal conflict fact is written only after the app-owned copy
            // and its durable record have both been proved.
            let record = self
                .workspace_movements
                .get_mut(movement_id)
                .expect("movement was checked above");
            record.result_identity = Some(returned);
            record.status.state = NativeAgentWorkspaceMovementStateV1::ApplyingResult;
            record.status.code = Some("conflict_result_retention_pending".into());
            let status = record.status.clone();
            let task_id = record.status.task_id.clone();
            let _ = record;
            self.persist_envelope(&task_id, None)?;
            return Ok(status);
        }
        if record
            .result_identity
            .as_ref()
            .is_some_and(|existing| !same_logical_file_set(existing, &returned))
        {
            return invalid("Native Agent result Return does not match its exact durable result.");
        }
        self.apply_exact_workspace_result(movement_id, &source, returned_workspace, returned)
    }

    fn apply_exact_workspace_result(
        &mut self,
        movement_id: &str,
        source: &LocalMovementSourceV1,
        returned_workspace: &Path,
        returned_identity: crate::safe_file_identity::RegularFileSetIdentity,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        self.apply_exact_workspace_result_with_crash(
            movement_id,
            source,
            returned_workspace,
            returned_identity,
            None,
        )
    }

    #[allow(dead_code)]
    fn apply_exact_workspace_result_with_crash(
        &mut self,
        movement_id: &str,
        source: &LocalMovementSourceV1,
        returned_workspace: &Path,
        returned_identity: crate::safe_file_identity::RegularFileSetIdentity,
        simulated_crash: Option<ApplyCrashPointV1>,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let has_prior_transaction = self
            .workspace_movements
            .get(movement_id)
            .is_some_and(|record| record.apply_transaction.is_some());
        if has_prior_transaction {
            let recovery_result = {
                let record = self
                    .workspace_movements
                    .get_mut(movement_id)
                    .expect("movement was checked above");
                if record
                    .apply_transaction
                    .as_ref()
                    .is_some_and(|transaction| {
                        !same_logical_file_set(&transaction.result_identity, &returned_identity)
                    })
                {
                    return invalid(
                        "Native Agent result Return does not match its durable apply transaction.",
                    );
                }
                recover_workspace_apply_record(movement_id, record)
            };
            if let Err(error) = recovery_result {
                return Err(error);
            }
            let task_id = self
                .workspace_movements
                .get(movement_id)
                .expect("movement was checked above")
                .status
                .task_id
                .clone();
            self.persist_envelope(&task_id, None)?;
            let record = self
                .workspace_movements
                .get(movement_id)
                .expect("movement was checked above");
            if record.apply_completed {
                return Ok(record.status.clone());
            }
        }
        let (stage, backup) = apply_transaction_paths(&source.workspace, movement_id)?;
        if identity_at(&stage)?.is_some() || identity_at(&backup)?.is_some() {
            return invalid(
                "Native Agent apply transaction paths are already occupied by another tree.",
            );
        }
        let task_id = {
            let record = self
                .workspace_movements
                .get_mut(movement_id)
                .ok_or_else(|| {
                    AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
                })?;
            record.result_identity = Some(returned_identity.clone());
            record.status.state = NativeAgentWorkspaceMovementStateV1::ApplyingResult;
            record.status.code = None;
            record.apply_transaction = Some(WorkspaceApplyTransactionV1 {
                result_identity: returned_identity.clone(),
                phase: WorkspaceApplyPhaseV1::Staging,
            });
            record.status.task_id.clone()
        };
        self.persist_envelope(&task_id, None)?;
        simulate_apply_crash(simulated_crash, ApplyCrashPointV1::IntentPersisted)?;

        let applied = (|| {
            stage_exact_workspace_result(&stage, returned_workspace, &returned_identity)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::StageWritten)?;
            self.workspace_movements
                .get_mut(movement_id)
                .expect("movement was checked above")
                .apply_transaction
                .as_mut()
                .expect("apply intent was persisted")
                .phase = WorkspaceApplyPhaseV1::Staged;
            self.persist_envelope(&task_id, None)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::StageJournaled)?;

            let observed = crate::safe_file_identity::capture_regular_file_set_identity(
                &source.workspace,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )?;
            if observed != source.baseline {
                return invalid("Native Agent source changed before result apply.");
            }
            fs::rename(&source.workspace, &backup)?;
            sync_directory(source.workspace.parent().ok_or_else(|| {
                AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
            })?)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::OriginalMoved)?;
            self.workspace_movements
                .get_mut(movement_id)
                .expect("movement was checked above")
                .apply_transaction
                .as_mut()
                .expect("apply intent was persisted")
                .phase = WorkspaceApplyPhaseV1::OriginalMoved;
            self.persist_envelope(&task_id, None)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::OriginalMoveJournaled)?;

            fs::rename(&stage, &source.workspace)?;
            sync_directory(source.workspace.parent().ok_or_else(|| {
                AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
            })?)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::ResultInstalled)?;
            let installed = crate::safe_file_identity::capture_regular_file_set_identity(
                &source.workspace,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )?;
            if !same_logical_file_set(&installed, &returned_identity) {
                return invalid("Native Agent result did not survive bounded apply.");
            }
            self.workspace_movements
                .get_mut(movement_id)
                .expect("movement was checked above")
                .apply_transaction
                .as_mut()
                .expect("apply intent was persisted")
                .phase = WorkspaceApplyPhaseV1::ResultInstalled;
            self.persist_envelope(&task_id, None)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::ResultInstallJournaled)?;

            {
                let record = self
                    .workspace_movements
                    .get_mut(movement_id)
                    .expect("movement was checked above");
                record.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
                record.status.code = None;
                record.apply_completed = true;
                record
                    .apply_transaction
                    .as_mut()
                    .expect("apply intent was persisted")
                    .phase = WorkspaceApplyPhaseV1::Committed;
            }
            self.persist_envelope(&task_id, None)?;
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::Committed)?;
            {
                let record = self
                    .workspace_movements
                    .get_mut(movement_id)
                    .expect("movement was checked above");
                let _ = finish_committed_apply_cleanup(movement_id, record);
            }
            simulate_apply_crash(simulated_crash, ApplyCrashPointV1::BackupCleaned)?;
            Ok(())
        })();

        if let Err(error) = applied {
            if simulated_crash.is_some() {
                return Err(error);
            }
            let record = self
                .workspace_movements
                .get_mut(movement_id)
                .expect("movement was checked above");
            if recover_workspace_apply_record(movement_id, record).is_err() {
                record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                record.status.code = Some("result_apply_interrupted".into());
            }
            let _ = self.persist_envelope(&task_id, None);
            return Err(error);
        }

        let status = self
            .workspace_movements
            .get(movement_id)
            .expect("movement was checked above")
            .status
            .clone();
        self.persist_envelope(&task_id, None)?;
        Ok(status)
    }

    fn finalize_conflict_recovery(
        &mut self,
        movement_id: &str,
        paths: &crate::storage::AppPaths,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        let result = record.result_identity.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent conflict result is unavailable.".into())
        })?;
        let conflict =
            crate::storage::get_native_agent_conflict(paths, movement_id)?.ok_or_else(|| {
                AppError::InvalidInput(
                    "Native Agent retained conflict result is unavailable.".into(),
                )
            })?;
        if conflict.task_id != record.status.task_id
            || conflict.result_digest != result.digest
            || conflict.result_byte_count != result.byte_count
        {
            return invalid("Native Agent retained conflict correlation is invalid.");
        }
        validate_retained_conflict(paths, &conflict)?;
        record.status.state = NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired;
        record.status.code = Some("source_changed_since_approval".into());
        let status = record.status.clone();
        let task_id = record.status.task_id.clone();
        let _ = record;
        self.persist_envelope(&task_id, None)?;
        Ok(status)
    }

    pub(crate) fn retained_conflict_result_for_reveal(
        &self,
        movement_id: &str,
    ) -> AppResult<PathBuf> {
        let paths = self.durable_paths.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent conflict persistence is unavailable.".into())
        })?;
        let movement = self.workspace_movements.get(movement_id).ok_or_else(|| {
            AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
        })?;
        if movement.status.state != NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired {
            return invalid("Native Agent conflict result is not available for reveal.");
        }
        let result = movement.result_identity.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent conflict result is unavailable.".into())
        })?;
        let conflict =
            crate::storage::get_native_agent_conflict(paths, movement_id)?.ok_or_else(|| {
                AppError::InvalidInput(
                    "Native Agent retained conflict result is unavailable.".into(),
                )
            })?;
        if conflict.task_id != movement.status.task_id
            || conflict.result_digest != result.digest
            || conflict.result_byte_count != result.byte_count
        {
            return invalid("Native Agent retained conflict correlation is invalid.");
        }
        validate_retained_conflict(paths, &conflict)
    }

    pub(crate) fn discard_retained_conflict_result(
        &mut self,
        movement_id: &str,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let paths = self.durable_paths.clone().ok_or_else(|| {
            AppError::InvalidInput("Native Agent conflict persistence is unavailable.".into())
        })?;
        let (task_id, was_pending, prior_code) = {
            let movement = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            if movement.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled
                && movement.status.code.as_deref() == Some("conflict_result_discarded")
            {
                return Ok(movement.status.clone());
            }
            (
                movement.status.task_id.clone(),
                movement.status.state == NativeAgentWorkspaceMovementStateV1::Cancelled
                    && movement.status.code.as_deref() == Some("conflict_result_discard_pending"),
                movement.status.code.clone(),
            )
        };
        if !was_pending {
            let movement = self
                .workspace_movements
                .get(movement_id)
                .expect("checked above");
            if movement.status.state
                != NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
            {
                return invalid("Native Agent conflict result is not available for discard.");
            }
            let result = movement.result_identity.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Native Agent conflict result is unavailable.".into())
            })?;
            let conflict = crate::storage::get_native_agent_conflict(&paths, movement_id)?
                .ok_or_else(|| {
                    AppError::InvalidInput(
                        "Native Agent retained conflict result is unavailable.".into(),
                    )
                })?;
            if conflict.task_id != task_id
                || conflict.result_digest != result.digest
                || conflict.result_byte_count != result.byte_count
            {
                return invalid("Native Agent retained conflict correlation is invalid.");
            }
            // Eligibility is proved before the durable pending marker. Later
            // retries accept a missing tree only because this exact marker
            // records that deletion may already have completed.
            validate_retained_conflict(&paths, &conflict)?;
            {
                let movement = self
                    .workspace_movements
                    .get_mut(movement_id)
                    .expect("checked above");
                movement.status.state = NativeAgentWorkspaceMovementStateV1::Cancelled;
                movement.status.code = Some("conflict_result_discard_pending".into());
            }
            if let Err(error) = self.persist_envelope(&task_id, None) {
                let movement = self
                    .workspace_movements
                    .get_mut(movement_id)
                    .expect("checked above");
                movement.status.state =
                    NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired;
                movement.status.code = prior_code;
                return Err(error);
            }
        }
        self.finish_pending_conflict_discard(movement_id)
    }

    /// Completes only a cancellation whose pending marker was already durable.
    /// This is deliberately not a movement transition framework: it is the
    /// recoverable consequence of one explicit retained-result discard.
    fn finish_pending_conflict_discard(
        &mut self,
        movement_id: &str,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let paths = self.durable_paths.clone().ok_or_else(|| {
            AppError::InvalidInput("Native Agent conflict persistence is unavailable.".into())
        })?;
        let (task_id, result) = {
            let movement = self.workspace_movements.get(movement_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
            if movement.status.state != NativeAgentWorkspaceMovementStateV1::Cancelled
                || movement.status.code.as_deref() != Some("conflict_result_discard_pending")
            {
                return invalid("Native Agent conflict discard is not pending.");
            }
            (
                movement.status.task_id.clone(),
                movement.result_identity.clone().ok_or_else(|| {
                    AppError::InvalidInput("Native Agent conflict result is unavailable.".into())
                })?,
            )
        };
        if let Some(conflict) = crate::storage::get_native_agent_conflict(&paths, movement_id)? {
            if conflict.task_id != task_id
                || conflict.result_digest != result.digest
                || conflict.result_byte_count != result.byte_count
            {
                return invalid("Native Agent retained conflict correlation is invalid.");
            }
            delete_exact_retained_conflict_container(&paths, &conflict)?;
            crate::storage::delete_native_agent_conflict(&paths, movement_id)?;
        }
        let movement = self
            .workspace_movements
            .get_mut(movement_id)
            .expect("checked above");
        movement.status.code = Some("conflict_result_discarded".into());
        let _ = movement;
        if let Err(error) = self.persist_envelope(&task_id, None) {
            self.workspace_movements
                .get_mut(movement_id)
                .expect("checked above")
                .status
                .code = Some("conflict_result_discard_pending".into());
            return Err(error);
        }
        self.movement_status(movement_id)
    }

    /// Continues this Host user's Codex conversation in `workspace` (or
    /// begins one), or with `new_conversation` explicitly begins a new one.
    pub(crate) fn start_codex_conversation_task(
        &mut self,
        workspace: &Path,
        task: &str,
        new_conversation: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        self.start_codex_task_with_id_resume(
            &format!("native-agent:{}", Uuid::new_v4()),
            &workspace,
            task,
            !new_conversation,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_codex_task_with_id(
        &mut self,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty() || task_id.len() > MAX_TASK_ID_BYTES {
            return invalid("Native Agent task identity is invalid.");
        }
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            self.require_local_task_owner(task_id)?;
            if self.task_workspaces.get(task_id) != Some(&workspace)
                || self.task_digests.get(task_id) != Some(&Self::task_digest(task))
            {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        self.start_codex_task_with_id_resume(task_id, &workspace, task, true)
    }

    pub(crate) fn start_codex_task_with_id_resume(
        &mut self,
        task_id: &str,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty() || task_id.len() > MAX_TASK_ID_BYTES {
            return invalid("Native Agent task identity is invalid.");
        }
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return invalid("Native Agent task is empty or exceeds its bound.");
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if !workspace.is_dir() {
            return invalid("Native Agent workspace must be a directory.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            self.require_local_task_owner(task_id)?;
            if self.task_workspaces.get(task_id) != Some(&workspace)
                || self.task_digests.get(task_id) != Some(&Self::task_digest(task))
            {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        self.start_codex_task_with_executable_id_and_resume(
            Path::new("codex"),
            task_id,
            &workspace,
            task,
            resume,
        )
    }

    #[cfg(test)]
    fn start_codex_task_with_executable(
        &mut self,
        executable: &Path,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_codex_task_with_executable_and_id(
            executable,
            &format!("native-agent:{}", Uuid::new_v4()),
            workspace,
            task,
        )
    }

    #[cfg(test)]
    fn start_codex_task_with_executable_and_id(
        &mut self,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_codex_task_with_executable_id_and_resume(
            executable, task_id, workspace, task, true,
        )
    }

    /// `resume` continues this owner's conversation in the workspace, or
    /// begins one when none exists; `false` explicitly begins a new one.
    fn start_codex_task_with_executable_id_and_resume(
        &mut self,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.codex.use_executable(executable);
        let input = capability::codex::codex_input(workspace, task, resume)?;
        self.start_local_invocation(CODEX_CAPABILITY_ID, task_id, &input)
    }

    /// Continues one existing Claude Code session that this Host's user
    /// chose, in its canonical workspace, with `prompt`. Local only; the
    /// adapter refuses while any process holds the session.
    pub(crate) fn start_claude_session_task(
        &mut self,
        workspace: &Path,
        session_id: &str,
        prompt: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let input = capability::claude_code::continue_input(session_id, workspace, prompt)?;
        self.start_local_invocation(
            capability::claude_code::CLAUDE_CODE_CAPABILITY_ID,
            &format!("native-agent:{}", Uuid::new_v4()),
            &input,
        )
    }

    /// Continues one existing Codex thread in its own native session. The
    /// thread must still end with `expected_last_turn_id` once Codex's
    /// writer lock is held; another client holding it refuses the start.
    pub(crate) fn start_codex_thread_task(
        &mut self,
        workspace: &Path,
        thread_id: &str,
        prompt: &str,
        expected_last_turn_id: Option<&str>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let input = capability::codex::thread_continuation::continue_input(
            workspace,
            thread_id,
            prompt,
            expected_last_turn_id,
        )?;
        self.start_local_invocation(
            capability::codex::thread_continuation::CODEX_THREAD_CAPABILITY_ID,
            &format!("native-agent:{}", Uuid::new_v4()),
            &input,
        )
    }

    /// An invocation by this Host's own user, admitted by the same policy as
    /// a remote one. A replay keeps the existing task identity rules.
    fn start_local_invocation(
        &mut self,
        capability_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.require_local_task_owner(task_id)?;
        if self.task_owner(task_id)?.is_some() {
            return self.start_invocation_in_movement(
                capability_id,
                task_id,
                input,
                None,
                Some(NativeConversationOwnerV1::Local),
            );
        }
        self.admit_invocation(
            NativeInvocationPrincipalV1::Local,
            capability_id,
            task_id,
            input,
            NativeInvocationContinuationV1::Direct,
            crate::storage::now_ts(),
        )
    }

    /// Receives `native_agent.invoke` from the exact authenticated current
    /// peer session `binding`; the caller has already checked that this Host
    /// is the target. Then, in order: immutable task ownership and replay,
    /// capability availability, admission. A remote invocation waits for
    /// Review, and no adapter sees its input before Accept.
    pub(crate) fn receive_bridge_invocation(
        &mut self,
        binding: &HostSessionBinding,
        request: &NativeInvocationRequestV1,
        now: i64,
        verified_device: Option<&crate::device_identity::VerifiedPeerDevice>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if !valid_task_id(&request.task_id) {
            return invalid("Native Agent task identity is invalid.");
        }
        request.input.validate()?;
        let first_request = !self.task_bridges.contains_key(&request.task_id);
        let requester = verified_device
            .filter(|device| device.binding.validate_current(binding, now).is_ok())
            .map(|device| NativeTaskRequesterV1 {
                bridge_id: binding.bridge_id.clone(),
                peer_host_ref: binding.peer_host_ref.as_str().to_owned(),
                fingerprint: device.fingerprint.clone(),
            });
        // Only the request that creates the task names its requester, and
        // before anything can start, so its native requests know who may
        // decide them; a replay never adds or replaces one.
        let recorded = match (first_request, requester) {
            (true, Some(requester)) => {
                self.task_requesters
                    .insert(request.task_id.clone(), requester);
                true
            }
            _ => false,
        };
        let status = self.with_bridge_task_owner(&binding.bridge_id, &request.task_id, |service| {
            if let Some(existing) = service.replayed_invocation(
                &request.task_id,
                &request.agent_capability,
                &request.input,
            )? {
                return Ok(existing);
            }
            if let Some(device) = verified_device {
                if let Some(started) =
                    service.start_under_unattended_grant(binding, device, request, now)?
                {
                    return Ok(started);
                }
            }
            service.admit_invocation(
                NativeInvocationPrincipalV1::Remote(binding),
                &request.agent_capability,
                &request.task_id,
                &request.input,
                NativeInvocationContinuationV1::Direct,
                now,
            )
        });
        if recorded && status.is_err() && !self.task_bridges.contains_key(&request.task_id) {
            self.task_requesters.remove(&request.task_id);
        }
        log_native_lifecycle(
            "invocation_received",
            &request.task_id,
            &match &status {
                Ok(admitted) => format!(
                    "{} first={first_request} requester_verified={}",
                    lifecycle_state(admitted),
                    verified_device.is_some()
                ),
                Err(_) => format!(
                    "rejected first={first_request} requester_verified={}",
                    verified_device.is_some()
                ),
            },
        );
        status
    }

    /// The requesting user's explicit decision on one native request of
    /// their running task. Accepted only over a current session of the
    /// task's Bridge, from the same peer Host and the same device key that
    /// requested the task, proven in this session; only for a review still
    /// pending, unexpired, shown with exactly this content and offering this
    /// decision. An unattended permission plays no part: it never decides a
    /// native request. Nothing here waits for the user or the network.
    pub(crate) fn submit_native_decision(
        &mut self,
        binding: &HostSessionBinding,
        verified_device: Option<&crate::device_identity::VerifiedPeerDevice>,
        request: &NativeAgentDecisionV1,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        validate_decision(request)?;
        if request.target_host_ref != binding.local_host_ref.as_str() {
            return invalid("Native Agent decision names another Host.");
        }
        if self.revoked_bridges.contains(&binding.bridge_id)
            || self.task_bridges.get(&request.task_id) != Some(&binding.bridge_id)
        {
            return invalid("Native Agent decision crossed its Bridge authority.");
        }
        let requester = self.task_requesters.get(&request.task_id).ok_or_else(|| {
            AppError::InvalidInput(
                "Only the device that started this task can decide its requests.".into(),
            )
        })?;
        let device = verified_device
            .filter(|device| device.binding.validate_current(binding, now).is_ok())
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "This device has not proven its identity in this session.".into(),
                )
            })?;
        if requester.bridge_id != binding.bridge_id
            || requester.peer_host_ref != binding.peer_host_ref.as_str()
            || requester.fingerprint != device.fingerprint
        {
            return invalid("Only the device that started this task can decide its requests.");
        }
        let agent_id = {
            let tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let status = tasks
                .get(&request.task_id)
                .filter(|status| status.state == NativeAgentTaskStateV1::Running)
                .ok_or_else(|| AppError::InvalidInput("This task is no longer running.".into()))?;
            let review = status
                .native_decisions
                .iter()
                .find(|review| review.review_ref == request.review_ref)
                .ok_or_else(|| {
                    AppError::InvalidInput("Codex no longer waits for this decision.".into())
                })?;
            let offered = match request.decision {
                NativeDecisionV1::Accept => review.can_accept,
                NativeDecisionV1::Decline => review.can_decline,
            };
            if review.state != NativeDecisionStateV1::Pending {
                return invalid("This decision was already given.");
            }
            if review.expires_at <= now {
                return invalid("This request expired.");
            }
            if review.content_digest != request.content_digest {
                return invalid("What Codex asks changed; review it again.");
            }
            if !offered {
                return invalid("Codex does not offer this decision for this request.");
            }
            status.agent_id.clone()
        };
        self.adapter(&agent_id)?.respond_native_decision(
            &request.task_id,
            &request.review_ref,
            &request.content_digest,
            request.decision,
            now,
        )?;
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let status = tasks
            .get_mut(&request.task_id)
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))?;
        // Codex may already have reported it resolved; that fact stands.
        if let Some(review) = status
            .native_decisions
            .iter_mut()
            .find(|review| review.review_ref == request.review_ref)
            .filter(|review| review.state == NativeDecisionStateV1::Pending)
        {
            review.state = NativeDecisionStateV1::Submitted;
            review.submitted = Some(request.decision);
            review.can_accept = false;
            review.can_decline = false;
        }
        Ok(status.clone())
    }

    /// A replay of an existing task answers with that task. Identity is the
    /// exact input Core admitted, so a replayed input never reaches an
    /// adapter. A task whose admitted input is no longer known (restored
    /// after restart) refuses every replay; none could start it again.
    fn replayed_invocation(
        &self,
        task_id: &str,
        capability_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<Option<NativeAgentTaskStatusV1>> {
        let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        else {
            return Ok(None);
        };
        if existing.agent_id != capability_id
            || self.task_input_digests.get(task_id) != Some(&input.digest())
        {
            return invalid("Native Agent task identity was replayed with different input.");
        }
        Ok(Some(existing))
    }

    /// The one admission point for a new invocation, direct or a received
    /// workspace. Core admits; the adapter only validates and starts what
    /// Core admitted.
    fn admit_invocation(
        &mut self,
        principal: NativeInvocationPrincipalV1<'_>,
        capability_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        continuation: NativeInvocationContinuationV1,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let adapter = self.adapter(capability_id)?;
        // A capability that advertises no peer protocol is local to this
        // Host: no Bridge peer invokes it, whatever it names.
        if matches!(principal, NativeInvocationPrincipalV1::Remote(_))
            && adapter.supported_protocols().is_empty()
        {
            return invalid("This native capability is local to its Host.");
        }
        adapter.require_available()?;
        match (admission::decide(&principal), principal) {
            (NativeAdmissionDecisionV1::Allow, _) => match continuation {
                NativeInvocationContinuationV1::Direct => {
                    // Only the Local principal is admitted without Review.
                    self.start_invocation_in_movement(
                        capability_id,
                        task_id,
                        input,
                        None,
                        Some(NativeConversationOwnerV1::Local),
                    )
                }
                NativeInvocationContinuationV1::ReceivedWorkspace { movement_id } => {
                    self.start_admitted_received_workspace(&movement_id, task_id, input)
                }
            },
            (
                NativeAdmissionDecisionV1::RequireReview,
                NativeInvocationPrincipalV1::Remote(binding),
            ) => self.hold_for_review(binding, capability_id, task_id, input, continuation, now),
            _ => invalid("Native capability invocation was not admitted by this Host."),
        }
    }

    /// Starts a remote invocation without Review only under an active grant
    /// this Host's user created for exactly this verified device key, Bridge,
    /// peer, Host, capability and canonical workspace, spending one use
    /// atomically right before the start. `None` when no grant applies: the
    /// invocation then waits for Review as before. A grant authorizes
    /// invoking the capability only; the capability's own approvals are
    /// never answered.
    fn start_under_unattended_grant(
        &mut self,
        binding: &HostSessionBinding,
        device: &crate::device_identity::VerifiedPeerDevice,
        request: &NativeInvocationRequestV1,
        now: i64,
    ) -> AppResult<Option<NativeAgentTaskStatusV1>> {
        let Some(paths) = self.durable_paths.clone() else {
            return Ok(None);
        };
        // The identity proof must belong to exactly this session.
        if device.binding.validate_current(binding, now).is_err()
            || self.revoked_bridges.contains(&binding.bridge_id)
        {
            return Ok(None);
        }
        let public_key = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            device.public_key,
        );
        let grants = crate::storage::candidate_unattended_grants(
            &paths,
            &device.fingerprint,
            &public_key,
            &binding.bridge_id,
            binding.peer_host_ref.as_str(),
            binding.local_host_ref.as_str(),
            &request.agent_capability,
            now,
        )?;
        if grants.is_empty() {
            return Ok(None);
        }
        let adapter = self.adapter(&request.agent_capability)?;
        adapter.require_available()?;
        let prepared = adapter.prepare(&request.input)?;
        let Some(workspace) = prepared.host_workspace.clone() else {
            return Ok(None);
        };
        // The grant's folder must still resolve to exactly what was granted.
        let Some(grant) = grants.iter().find(|grant| {
            let granted = PathBuf::from(&grant.workspace);
            granted == workspace && granted.canonicalize().ok().as_ref() == Some(&granted)
        }) else {
            return Ok(None);
        };
        // A busy workspace refuses before a use is spent.
        if self.invocation_has_authoritative_owner(
            prepared.exclusivity.as_ref(),
            Some(&workspace),
            None,
            None,
        )? {
            return invalid(
                "Native capability already has an unresolved invocation for this resource.",
            );
        }
        if !crate::storage::consume_unattended_grant(
            &paths,
            &grant.grant_id,
            &request.task_id,
            now,
        )? {
            return Ok(None);
        }
        self.start_invocation_in_scope(
            &request.agent_capability,
            &request.task_id,
            &request.input,
            None,
            Some(NativeConversationOwnerV1::Peer {
                bridge_id: binding.bridge_id.clone(),
                peer_host_ref: binding.peer_host_ref.as_str().to_owned(),
            }),
            Some(&workspace),
        )
        .map(Some)
    }

    /// Holds a remote invocation for Review as a Queued task; no adapter is
    /// called. A direct invocation's task is new and nothing about it is
    /// durable. A received workspace's task already exists and keeps its
    /// envelope, which owns the landed tree; only its code changes.
    fn hold_for_review(
        &mut self,
        binding: &HostSessionBinding,
        capability_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        continuation: NativeInvocationContinuationV1,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.expire_invocation_reviews(now);
        let expires_at = (now + admission::INVOCATION_REVIEW_TTL_SECONDS).min(binding.expires_at);
        if expires_at <= now {
            return invalid("Native Agent Bridge session has expired.");
        }
        if self
            .pending_reviews
            .values()
            .filter(|pending| pending.binding.bridge_id == binding.bridge_id)
            .count()
            >= admission::MAX_PENDING_REVIEWS_PER_BRIDGE
        {
            return invalid("Too many native capability invocations are awaiting Review.");
        }
        let status = match &continuation {
            NativeInvocationContinuationV1::Direct => {
                let status = NativeAgentTaskStatusV1 {
                    schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                    task_id: task_id.into(),
                    agent_id: capability_id.into(),
                    workspace_name: capability_id.into(),
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Queued,
                    result: None,
                    code: Some(admission::REVIEW_REQUIRED_CODE.into()),
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                };
                self.tasks
                    .lock()
                    .map_err(|_| {
                        AppError::InvalidInput("Native Agent task store is unavailable.".into())
                    })?
                    .insert(task_id.into(), status.clone());
                self.unstarted_review_tasks.insert(task_id.into());
                self.task_input_digests
                    .insert(task_id.into(), input.digest());
                status
            }
            NativeInvocationContinuationV1::ReceivedWorkspace { .. } => {
                let previous = {
                    let mut tasks = self.tasks.lock().map_err(|_| {
                        AppError::InvalidInput("Native Agent task store is unavailable.".into())
                    })?;
                    let task = tasks
                        .get_mut(task_id)
                        .filter(|task| task.state == NativeAgentTaskStateV1::Queued)
                        .ok_or_else(|| {
                            AppError::InvalidInput(
                                "Native Agent workspace task is not awaiting its start.".into(),
                            )
                        })?;
                    task.code.replace(admission::REVIEW_REQUIRED_CODE.into())
                };
                self.unstarted_review_tasks.insert(task_id.into());
                if let Err(error) = self.persist_envelope(task_id, None) {
                    self.unstarted_review_tasks.remove(task_id);
                    if let Some(task) = self
                        .tasks
                        .lock()
                        .map_err(|_| {
                            AppError::InvalidInput("Native Agent task store is unavailable.".into())
                        })?
                        .get_mut(task_id)
                    {
                        task.code = previous;
                    }
                    return Err(error);
                }
                self.task_status(task_id)?
            }
        };
        self.pending_reviews.insert(
            task_id.into(),
            PendingNativeInvocationV1 {
                task_id: task_id.into(),
                capability_id: capability_id.into(),
                binding: binding.clone(),
                expires_at,
                input: input.clone(),
                continuation,
            },
        );
        Ok(status)
    }

    /// Ends every Review whose time has passed. Evaluated whenever Reviews
    /// are read or decided, and by the invocation's existing status loop.
    pub(crate) fn expire_invocation_reviews(&mut self, now: i64) {
        let expired = self
            .pending_reviews
            .values()
            .filter(|pending| pending.expires_at <= now)
            .map(|pending| pending.task_id.clone())
            .collect::<Vec<_>>();
        for task_id in expired {
            self.pending_reviews.remove(&task_id);
            let _ = self.end_review(&task_id, admission::REVIEW_EXPIRED_CODE);
        }
    }

    /// Ends a Review-held task as a definite non-start. A received workspace
    /// movement ends with it, and the existing terminal cleanup removes its
    /// landed tree. Nothing was prepared or started, so nothing needs
    /// reconciliation.
    fn end_review(&mut self, task_id: &str, code: &str) -> AppResult<NativeAgentTaskStatusV1> {
        self.review_share_requirements.remove(task_id);
        let status = self.finish_unstarted_review(task_id, code)?;
        let movement_id = self
            .workspace_movements
            .iter()
            .find(|(_, record)| {
                record.status.task_id == task_id && record.prepared_remote.is_some()
            })
            .map(|(movement_id, _)| movement_id.clone());
        if let Some(movement_id) = movement_id {
            if let Some(record) = self.workspace_movements.get_mut(&movement_id) {
                if record.status.state == NativeAgentWorkspaceMovementStateV1::TransferringToAgent
                    && status.state == NativeAgentTaskStateV1::Failed
                {
                    record.status.state = NativeAgentWorkspaceMovementStateV1::Failed;
                    record.status.code = status.code.clone();
                }
            }
            self.persist_envelope(task_id, None)?;
            self.cleanup_terminal_remote_workspace(&movement_id)?;
        }
        Ok(status)
    }

    /// Records the definite non-start of a Review-held task. A cancellation
    /// that already ended it stands.
    fn finish_unstarted_review(
        &mut self,
        task_id: &str,
        code: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))?;
        if task.state == NativeAgentTaskStateV1::Queued {
            task.state = NativeAgentTaskStateV1::Failed;
            task.native_decisions.clear();
            task.code = Some(code.into());
        }
        Ok(task.clone())
    }

    /// Renderer-safe projection of every pending Review.
    pub(crate) fn invocation_reviews(&mut self, now: i64) -> Vec<NativeInvocationReviewV1> {
        self.expire_invocation_reviews(now);
        let mut reviews = self
            .pending_reviews
            .values()
            .map(|pending| {
                let display_name = self
                    .adapter(&pending.capability_id)
                    .map(|adapter| adapter.display_name().to_owned())
                    .unwrap_or_else(|_| pending.capability_id.clone());
                let shared_folder = self
                    .review_share_requirements
                    .get(&pending.task_id)
                    .map(crate::session_read_access::ShareRequirementV1::folder_label);
                pending.review(&display_name, shared_folder)
            })
            .collect::<Vec<_>>();
        reviews.sort_by(|a, b| (a.expires_at, &a.task_id).cmp(&(b.expires_at, &b.task_id)));
        reviews
    }

    /// The Bridge and peer route whose current session an Accept of this
    /// Review must re-resolve.
    pub(crate) fn invocation_review_route(&self, task_id: &str) -> Option<(String, String)> {
        self.pending_reviews.get(task_id).map(|pending| {
            (
                pending.binding.bridge_id.clone(),
                pending.binding.peer_route_ref.clone(),
            )
        })
    }

    /// Host Accept of one pending Review. The Review is consumed whatever
    /// happens. `current` is the freshly resolved binding of the Review's
    /// own peer route, if there still is one. Only when the exact binding,
    /// the Bridge, the TTL, ownership and the capability all still hold does
    /// the adapter first see the input, to prepare and then start.
    pub(crate) fn accept_invocation_review(
        &mut self,
        task_id: &str,
        current: Option<&HostSessionBinding>,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.expire_invocation_reviews(now);
        let Some(pending) = self.pending_reviews.remove(task_id) else {
            log_native_lifecycle("review_accept_unavailable", task_id, "");
            return invalid("Native capability Review is unavailable.");
        };
        log_native_lifecycle("review_accepted", task_id, "");
        if let Err(error) = self.revalidate_review(&pending, current, now) {
            self.end_review(task_id, admission::ADMISSION_REVOKED_CODE)?;
            return Err(error);
        }
        // Admitted through a shared folder: that share must still allow it
        // now. Stopping the share, turning its switch off, its expiry,
        // revoking the device or Burn while the Review waited all end it.
        if let Some(requirement) = self.review_share_requirements.remove(task_id) {
            let holds = self.durable_paths.as_ref().is_some_and(|paths| {
                crate::session_read_access::share_requirement_holds(paths, &requirement, now)
                    .unwrap_or(false)
            });
            if !holds {
                log_native_lifecycle("review_share_revoked", task_id, "");
                self.end_review(task_id, admission::ADMISSION_REVOKED_CODE)?;
                return invalid(
                    "The folder is no longer shared for this request, so nothing was started.",
                );
            }
        }
        if let NativeInvocationContinuationV1::ReceivedWorkspace { movement_id } =
            &pending.continuation
        {
            if let Err(error) = self.revalidate_received_workspace(&pending, movement_id) {
                self.end_review(task_id, admission::ADMISSION_REVOKED_CODE)?;
                return Err(error);
            }
            // Then the existing movement lifecycle, unchanged.
            return match self.start_admitted_received_workspace(
                movement_id,
                task_id,
                &pending.input,
            ) {
                Ok(status) => Ok(status),
                Err(error) => {
                    let _ = self.end_review(task_id, admission::START_REJECTED_CODE);
                    Err(error)
                }
            };
        }
        // The placeholder gives way to the started task.
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .remove(task_id);
        self.unstarted_review_tasks.remove(task_id);
        // The Review's exact authenticated peer owns the conversation.
        match self.start_invocation_in_movement(
            &pending.capability_id,
            task_id,
            &pending.input,
            None,
            Some(NativeConversationOwnerV1::Peer {
                bridge_id: pending.binding.bridge_id.clone(),
                peer_host_ref: pending.binding.peer_host_ref.as_str().to_owned(),
            }),
        ) {
            Ok(status) => Ok(status),
            Err(error) => {
                let mut tasks = self.tasks.lock().map_err(|_| {
                    AppError::InvalidInput("Native Agent task store is unavailable.".into())
                })?;
                if !tasks.contains_key(task_id) {
                    // Nothing started: a definite, still non-durable non-start.
                    tasks.insert(
                        task_id.into(),
                        NativeAgentTaskStatusV1 {
                            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                            task_id: task_id.into(),
                            agent_id: pending.capability_id.clone(),
                            workspace_name: pending.capability_id.clone(),
                            session_reused: false,
                            state: NativeAgentTaskStateV1::Failed,
                            result: None,
                            code: Some(admission::START_REJECTED_CODE.into()),
                            output: None,
                            conversation: None,
                            native_decisions: Vec::new(),
                        },
                    );
                    self.unstarted_review_tasks.insert(task_id.into());
                }
                Err(error)
            }
        }
    }

    /// Records that a Review-held task came through `requirement`'s share.
    /// Only for a task still waiting for its Review; a task that started
    /// at once (an unattended grant) or already ended needs no later check.
    pub(crate) fn require_share_at_accept(
        &mut self,
        task_id: &str,
        requirement: crate::session_read_access::ShareRequirementV1,
    ) {
        if self.pending_reviews.contains_key(task_id) {
            self.review_share_requirements
                .entry(task_id.into())
                .or_insert(requirement);
        }
    }

    /// Whether `bridge_id` holds as many Reviews as it may.
    pub(crate) fn review_capacity_reached(&self, bridge_id: &str) -> bool {
        self.pending_reviews
            .values()
            .filter(|pending| pending.binding.bridge_id == bridge_id)
            .count()
            >= admission::MAX_PENDING_REVIEWS_PER_BRIDGE
    }

    /// Reviews of invocations `bridge_id`'s peers asked of this Host, by
    /// outcome, for diagnostics. Counts only.
    pub(crate) fn review_counts(&self, bridge_id: &str) -> NativeReviewCountsV1 {
        let mut counts = NativeReviewCountsV1 {
            pending: self
                .pending_reviews
                .values()
                .filter(|pending| pending.binding.bridge_id == bridge_id)
                .count(),
            limit: admission::MAX_PENDING_REVIEWS_PER_BRIDGE,
            ..NativeReviewCountsV1::default()
        };
        let Ok(tasks) = self.tasks.lock() else {
            return counts;
        };
        for (task_id, task) in tasks.iter() {
            if self.task_bridges.get(task_id).map(String::as_str) != Some(bridge_id)
                || self.remote_targets.contains_key(task_id)
                || self.pending_reviews.contains_key(task_id)
            {
                continue;
            }
            match task.code.as_deref() {
                Some(admission::ADMISSION_DENIED_CODE) => counts.denied += 1,
                Some(admission::REVIEW_EXPIRED_CODE) => counts.expired += 1,
                Some(admission::ADMISSION_REVOKED_CODE) => counts.revoked += 1,
                Some(admission::REVIEW_REQUIRED_CODE) => {}
                _ => counts.started += 1,
            }
        }
        counts
    }

    fn revalidate_review(
        &self,
        pending: &PendingNativeInvocationV1,
        current: Option<&HostSessionBinding>,
        now: i64,
    ) -> AppResult<()> {
        if pending.expires_at <= now {
            return invalid("Native capability Review has expired.");
        }
        let current = current.ok_or_else(|| {
            AppError::InvalidInput("Native capability Review session is unavailable.".into())
        })?;
        pending.binding.validate_current(current, now)?;
        if self.revoked_bridges.contains(&pending.binding.bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        if self.task_owner(&pending.task_id)?
            != Some(TaskOwnerV1::Bridge(pending.binding.bridge_id.clone()))
        {
            return invalid("Native Agent task belongs to another authority.");
        }
        let task = self.task_status(&pending.task_id)?;
        if task.state != NativeAgentTaskStateV1::Queued
            || task.code.as_deref() != Some(admission::REVIEW_REQUIRED_CODE)
        {
            return invalid("Native capability Review is no longer pending.");
        }
        self.adapter(&pending.capability_id)?.require_available()
    }

    /// Host Deny of one pending Review: a definite non-start.
    pub(crate) fn deny_invocation_review(
        &mut self,
        task_id: &str,
        now: i64,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.expire_invocation_reviews(now);
        if self.pending_reviews.remove(task_id).is_none() {
            return invalid("Native capability Review is unavailable.");
        }
        self.end_review(task_id, admission::ADMISSION_DENIED_CODE)
    }

    /// Ends every Review held for `bridge_id` as a non-start.
    fn end_bridge_reviews(&mut self, bridge_id: &str, code: &str) {
        let ended = self
            .pending_reviews
            .values()
            .filter(|pending| pending.binding.bridge_id == bridge_id)
            .map(|pending| pending.task_id.clone())
            .collect::<Vec<_>>();
        for task_id in ended {
            self.pending_reviews.remove(&task_id);
            let _ = self.end_review(&task_id, code);
        }
        self.movement_bindings
            .retain(|_, binding| binding.bridge_id != bridge_id);
    }

    /// The capability-neutral start of an invocation that is already admitted.
    /// The adapter owns what the input means and how the invocation runs;
    /// Pastey owns task identity, exclusivity, the durable envelope and the
    /// terminal fact.
    fn start_invocation_in_movement(
        &mut self,
        capability_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        movement_id: Option<&str>,
        conversation_owner: Option<NativeConversationOwnerV1>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_invocation_in_scope(
            capability_id,
            task_id,
            input,
            movement_id,
            conversation_owner,
            None,
        )
    }

    /// `required_workspace` pins the exact canonical workspace an admission
    /// authorized (an unattended grant): the invocation starts only there,
    /// as resolved both before and by the adapter's start.
    fn start_invocation_in_scope(
        &mut self,
        capability_id: &str,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
        movement_id: Option<&str>,
        conversation_owner: Option<NativeConversationOwnerV1>,
        required_workspace: Option<&Path>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if !valid_task_id(task_id) {
            return invalid("Native Agent task identity is invalid.");
        }
        input.validate()?;
        let adapter = self.adapter(capability_id)?;
        let prepared = adapter.prepare(input)?;
        if required_workspace.is_some() && prepared.host_workspace.as_deref() != required_workspace
        {
            return invalid("Native Agent workspace is outside its authorized scope.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if existing.agent_id != capability_id
                || self.task_workspaces.get(task_id) != prepared.host_workspace.as_ref()
                || self.task_digests.get(task_id) != Some(&prepared.identity_digest)
            {
                return invalid("Native Agent task identity was replayed with different input.");
            }
            return Ok(existing);
        }
        adapter.require_available()?;
        if self.invocation_has_authoritative_owner(
            prepared.exclusivity.as_ref(),
            prepared.host_workspace.as_deref(),
            None,
            movement_id,
        )? {
            return invalid(
                "Native capability already has an unresolved invocation for this resource.",
            );
        }
        // A conversation belongs to exactly one owner, capability and scope.
        // Only a continuing invocation sees its saved continuation, and only
        // its own; a revoked Bridge's conversation is never reopened.
        let conversation = match (conversation_owner, prepared.conversation.as_ref()) {
            (Some(owner), Some(prepared_conversation)) => {
                if owner
                    .bridge_id()
                    .is_some_and(|bridge_id| self.revoked_bridges.contains(bridge_id))
                {
                    return invalid("Native Agent Bridge authority was revoked.");
                }
                Some((
                    NativeConversationBindingV1::new(
                        owner,
                        capability_id,
                        &prepared_conversation.scope,
                    )?,
                    prepared_conversation.continue_existing,
                ))
            }
            _ => None,
        };
        let native_conversation = match &conversation {
            Some((binding, continue_existing)) => Some(NativeConversationV1 {
                key: binding.key.clone(),
                continue_existing: *continue_existing,
                continuation: match (*continue_existing, self.durable_paths.as_ref()) {
                    (true, Some(paths)) => binding.saved_continuation(paths)?,
                    _ => None,
                },
            }),
            None => None,
        };
        let started = adapter.start(task_id, input, native_conversation)?;
        log_native_lifecycle(
            "turn_starting",
            task_id,
            &format!(
                "capability={capability_id} session_reused={}",
                started.session_reused
            ),
        );
        if required_workspace.is_some() && started.host_workspace.as_deref() != required_workspace {
            // Nothing has run: the adapter only recorded the task.
            adapter.release(task_id);
            return invalid("Native Agent workspace changed outside its authorized scope.");
        }
        let task_id = task_id.to_owned();
        self.task_input_digests
            .insert(task_id.clone(), input.digest());
        if let Some((binding, _)) = &conversation {
            if binding.owner.bridge_id().is_some() {
                self.task_conversations
                    .insert(task_id.clone(), binding.key.clone());
            }
        }
        let status = NativeAgentTaskStatusV1 {
            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
            task_id: task_id.clone(),
            agent_id: capability_id.into(),
            workspace_name: prepared.label.clone(),
            session_reused: started.session_reused,
            state: NativeAgentTaskStateV1::Running,
            result: None,
            code: None,
            output: None,
            conversation: started.conversation,
            native_decisions: Vec::new(),
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.clone(), status.clone());
        if let Some(workspace) = prepared.host_workspace.clone() {
            self.task_workspaces.insert(task_id.clone(), workspace);
        }
        self.task_digests
            .insert(task_id.clone(), prepared.identity_digest.clone());
        if let Some(key) = prepared.exclusivity.clone() {
            self.task_exclusivity.insert(task_id.clone(), key);
        }
        self.persist_envelope(&task_id, Some(&prepared.identity_digest))?;
        if let Some(key) = prepared.exclusivity.clone() {
            self.active_exclusivity
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent session store is unavailable.".into())
                })?
                .insert(key, task_id.clone());
        }
        self.observed_tasks
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .insert(task_id.clone());
        let tasks = self.tasks.clone();
        let durable_paths = self.durable_paths.clone();
        let active_exclusivity = self.active_exclusivity.clone();
        let observed_tasks = self.observed_tasks.clone();
        let held_key = prepared.exclusivity;
        let mut run = started.run;
        let progress = ObservedTaskProgressV1 {
            tasks: tasks.clone(),
            task_id: task_id.clone(),
            remote_decisions: self.task_requesters.contains_key(&task_id),
            conversation: conversation.map(|(binding, _)| binding),
            durable_paths: durable_paths.clone(),
        };
        thread::spawn(move || {
            let NativeInvocationOutcomeV1 {
                kind: outcome,
                summary,
                output,
            } = run.run(&progress);
            let mut persisted_status = None;
            if let Ok(mut tasks) = tasks.lock() {
                if let Some(status) = tasks.get_mut(&task_id) {
                    // The turn ended: nothing it asked can be answered now.
                    status.native_decisions.clear();
                    // Cancellation wins a concurrent late native completion.
                    // The observer still reaches this point to release its
                    // Host-private execution occupancy.
                    if status.state == NativeAgentTaskStateV1::Cancelled {
                        if outcome == NativeTurnOutcomeV1::Cancelled {
                            status.code = Some("native_agent_cancelled".into());
                        }
                    } else {
                        match outcome {
                            NativeTurnOutcomeV1::Completed => {
                                status.state = NativeAgentTaskStateV1::Completed;
                                status.code = None;
                                status.result = summary;
                                status.output = output;
                            }
                            NativeTurnOutcomeV1::Failed => {
                                status.state = NativeAgentTaskStateV1::Failed;
                                status.code = Some("native_agent_failed".into());
                            }
                            NativeTurnOutcomeV1::Interrupted => {
                                status.state = NativeAgentTaskStateV1::Interrupted;
                                status.code = Some("native_agent_interrupted".into());
                            }
                            NativeTurnOutcomeV1::Cancelled => {
                                status.state = NativeAgentTaskStateV1::Cancelled;
                                status.code = Some("native_agent_cancelled".into());
                            }
                            NativeTurnOutcomeV1::Unknown => {
                                status.state = NativeAgentTaskStateV1::Interrupted;
                                status.code = Some("native_agent_outcome_unknown".into());
                            }
                            NativeTurnOutcomeV1::NotStarted => {
                                status.state = NativeAgentTaskStateV1::Failed;
                                status.code = Some(START_FAILED_CODE.into());
                                status.result = summary;
                            }
                            NativeTurnOutcomeV1::ConversationUnavailable => {
                                status.state = NativeAgentTaskStateV1::Failed;
                                status.code = Some(CONVERSATION_UNAVAILABLE_CODE.into());
                                status.result = summary;
                            }
                        }
                    }
                    persisted_status = Some(status.clone());
                }
            }
            log_native_lifecycle(
                "turn_ended",
                &task_id,
                &format!(
                    "outcome={outcome:?} {}",
                    persisted_status
                        .as_ref()
                        .map(lifecycle_state)
                        .unwrap_or_else(|| "state=unrecorded".into())
                ),
            );
            if let (Some(paths), Some(status)) = (durable_paths.as_ref(), persisted_status.as_ref())
            {
                NativeAgentServiceV1::persist_task_status_after_native_turn(
                    paths, &task_id, status,
                );
            }
            if outcome == NativeTurnOutcomeV1::Unknown {
                // A different task must not overlap possible execution.
                run.wait_until_observation_lost();
            }
            // The run lets go of what it holds first: its occupancy of a
            // reused session, or a continuation's own app-server and with it
            // Codex's writer lock. Only then may another task take the folder.
            drop(run);
            // Always release only after the native observer exits, including
            // when cancellation had already revoked visible authority.
            if let Some(key) = held_key {
                active_exclusivity
                    .lock()
                    .ok()
                    .map(|mut active| active.remove(&key));
            }
            observed_tasks
                .lock()
                .ok()
                .map(|mut observed| observed.remove(&task_id));
        });
        Ok(status)
    }

    /// Workspace movement's occupancy check for one canonical workspace.
    fn workspace_has_authoritative_owner(
        &self,
        workspace: &Path,
        except_task_id: Option<&str>,
        except_movement_id: Option<&str>,
    ) -> AppResult<bool> {
        let canonical = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        self.invocation_has_authoritative_owner(
            Some(&capability::codex::workspace_exclusivity_key(&canonical)),
            Some(&canonical),
            except_task_id,
            except_movement_id,
        )
    }

    /// The one Host-local occupancy check shared by direct invocations and
    /// approved workspace movement. A resource remains owned while any
    /// observed execution, movement, uncertain outcome, or apply recovery can
    /// still affect it. `key` is the adapter's opaque exclusivity key;
    /// `workspace` is the canonical workspace of workspace-scoped capabilities.
    fn invocation_has_authoritative_owner(
        &self,
        key: Option<&ExclusivityKeyV1>,
        workspace: Option<&Path>,
        except_task_id: Option<&str>,
        except_movement_id: Option<&str>,
    ) -> AppResult<bool> {
        if let Some(key) = key {
            if self
                .active_exclusivity
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent session store is unavailable.".into())
                })?
                .contains_key(key)
            {
                return Ok(true);
            }
        }
        if let Some(canonical) = workspace {
            if self
                .workspace_movements
                .iter()
                .any(|(movement_id, record)| {
                    if Some(movement_id.as_str()) == except_movement_id
                        || !movement_holds_source_ownership(&record.status)
                    {
                        return false;
                    }
                    record
                        .source
                        .as_ref()
                        .is_some_and(|source| source.workspace == canonical)
                        || record
                            .task_workspace
                            .as_ref()
                            .is_some_and(|owned| owned == canonical)
                })
            {
                return Ok(true);
            }
        }
        if key.is_none() && workspace.is_none() {
            return Ok(false);
        }
        let tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        Ok(tasks.iter().any(|(task_id, task)| {
            if Some(task_id.as_str()) == except_task_id {
                return false;
            }
            task.state == NativeAgentTaskStateV1::Interrupted
                && matches!(
                    task.code.as_deref(),
                    Some("native_agent_reconciliation_required")
                        | Some("native_agent_outcome_unknown")
                )
                && (key.is_some_and(|key| self.task_exclusivity.get(task_id) == Some(key))
                    || workspace.is_some_and(|canonical| {
                        self.task_workspaces
                            .get(task_id)
                            .is_some_and(|owned| owned == canonical)
                    }))
        }))
    }

    pub(crate) fn task_status(&self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))
    }

    /// Revokes the outer movement authority for the exact task. This says
    /// nothing about whether the Host-private native process has stopped; its
    /// workspace occupancy remains governed by `active_exclusivity`.
    fn cancel_matching_workspace_movement(&mut self, task_id: &str, code: &str) {
        for record in self
            .workspace_movements
            .values_mut()
            .filter(|record| record.status.task_id == task_id)
        {
            if !record.apply_completed
                && !matches!(
                    record.status.state,
                    NativeAgentWorkspaceMovementStateV1::Completed
                        | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                )
            {
                record.status.state = NativeAgentWorkspaceMovementStateV1::Cancelled;
                record.status.code = Some(code.into());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn queue_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.queue_remote_task_labelled(task_id, target_host_ref, workspace, None, task)
    }

    fn queue_remote_task_labelled(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        label: Option<&str>,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let label = label
            .map(str::trim)
            .filter(|label| {
                !label.is_empty() && label.len() <= 120 && !label.chars().any(char::is_control)
            })
            .unwrap_or_else(|| workspace.rsplit(['/', '\\']).next().unwrap_or("workspace"));
        if task_id.trim().is_empty()
            || target_host_ref.trim().is_empty()
            || workspace.trim().is_empty()
            || task.trim().is_empty()
        {
            return invalid("Remote native Agent task is invalid.");
        }
        self.queue_remote_invocation(
            task_id,
            target_host_ref,
            CODEX_CAPABILITY_ID,
            label,
            &Self::task_digest(task),
            Some(PathBuf::from(workspace)),
        )
    }

    /// Records one invocation this Host sent to another Host. The executor
    /// owns what `input` means; the requester keeps only its digest.
    pub(crate) fn queue_bridge_remote_invocation(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        target_host_ref: &str,
        capability_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if !valid_task_id(task_id) || target_host_ref.trim().is_empty() {
            return invalid("Remote native Agent task is invalid.");
        }
        crate::peer_capabilities::validate_semantic_capability_id(capability_id)?;
        input.validate()?;
        self.with_bridge_task_owner(bridge_id, task_id, |service| {
            service.queue_remote_invocation(
                task_id,
                target_host_ref,
                capability_id,
                capability_id,
                &input.digest(),
                None,
            )
        })
    }

    fn queue_remote_invocation(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        capability_id: &str,
        label: &str,
        identity_digest: &str,
        workspace: Option<PathBuf>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if existing.agent_id != capability_id
                || self.remote_targets.get(task_id).map(String::as_str) != Some(target_host_ref)
                || self.task_workspaces.get(task_id) != workspace.as_ref()
                || self.task_digests.get(task_id).map(String::as_str) != Some(identity_digest)
            {
                return invalid(
                    "Remote native Agent identity was reused with conflicting correlation.",
                );
            }
            return Ok(existing);
        }
        let status = NativeAgentTaskStatusV1 {
            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
            task_id: task_id.into(),
            agent_id: capability_id.into(),
            workspace_name: label.into(),
            session_reused: false,
            state: NativeAgentTaskStateV1::Queued,
            result: None,
            code: Some("remote_agent_queued".into()),
            output: None,
            conversation: None,
            native_decisions: Vec::new(),
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.into(), status.clone());
        self.remote_targets
            .insert(task_id.into(), target_host_ref.into());
        if let Some(workspace) = workspace {
            self.task_workspaces.insert(task_id.into(), workspace);
        }
        self.task_digests
            .insert(task_id.into(), identity_digest.into());
        self.persist_envelope(task_id, Some(identity_digest))?;
        Ok(status)
    }

    #[cfg(test)]
    pub(crate) fn queue_bridge_remote_task(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.queue_bridge_remote_task_labelled(
            bridge_id,
            task_id,
            target_host_ref,
            workspace,
            None,
            task,
        )
    }

    /// `label`: the folder's name when `workspace` is a shared folder's
    /// reference rather than a path.
    pub(crate) fn queue_bridge_remote_task_labelled(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        label: Option<&str>,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.with_bridge_task_owner(bridge_id, task_id, |service| {
            service.queue_remote_task_labelled(task_id, target_host_ref, workspace, label, task)
        })
    }

    pub(crate) fn record_remote_status(
        &mut self,
        remote: NativeAgentStatusV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        validate_status(&remote)?;
        if self.remote_targets.get(&remote.task_id).map(String::as_str)
            != Some(remote.executing_host_ref.as_str())
        {
            return invalid("Remote native Agent status crossed its selected Host binding.");
        }
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let current = tasks.get(&remote.task_id).cloned().ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if current.state == NativeAgentTaskStateV1::Cancelled {
            let status = current;
            drop(tasks);
            self.cancel_matching_workspace_movement(
                &remote.task_id,
                status.code.as_deref().unwrap_or("native_agent_cancelled"),
            );
            self.persist_envelope(&remote.task_id, None)?;
            return Ok(status);
        }
        let accepted_status = if task_accepts_remote_fact(&current, &remote.status) {
            tasks.insert(remote.task_id.clone(), remote.status.clone());
            remote.status.clone()
        } else {
            current
        };
        if let Some(record) = self
            .workspace_movements
            .values_mut()
            .find(|record| record.status.task_id == remote.task_id)
        {
            let next = match accepted_status.state {
                NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running => {
                    NativeAgentWorkspaceMovementStateV1::AgentRunning
                }
                NativeAgentTaskStateV1::Completed => {
                    NativeAgentWorkspaceMovementStateV1::ReturningResult
                }
                NativeAgentTaskStateV1::Failed => NativeAgentWorkspaceMovementStateV1::Failed,
                NativeAgentTaskStateV1::Cancelled => NativeAgentWorkspaceMovementStateV1::Cancelled,
                NativeAgentTaskStateV1::Interrupted => {
                    NativeAgentWorkspaceMovementStateV1::Interrupted
                }
            };
            if movement_accepts_remote_execution_fact(record, &next) {
                record.status.state = next;
                record.status.code = accepted_status.code.clone();
            }
        }
        drop(tasks);
        self.persist_envelope(&remote.task_id, None)?;
        Ok(accepted_status)
    }

    pub(crate) fn record_bridge_remote_status(
        &mut self,
        bridge_id: &str,
        remote: NativeAgentStatusV1,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(&remote.task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent status crossed its Bridge authority.");
        }
        self.record_remote_status(remote)
    }

    pub(crate) fn cancel_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.remote_targets.get(task_id).map(String::as_str) != Some(target_host_ref) {
            return invalid("Remote native Agent cancellation crossed its selected Host binding.");
        }
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks.get_mut(task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if matches!(
            task.state,
            NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
        ) || (task.state == NativeAgentTaskStateV1::Interrupted
            && matches!(
                task.code.as_deref(),
                Some("native_agent_reconciliation_required") | Some("native_agent_outcome_unknown")
            ))
        {
            task.state = NativeAgentTaskStateV1::Cancelled;
            task.native_decisions.clear();
            task.code = Some("native_agent_cancel_requested".into());
        }
        let status = task.clone();
        drop(tasks);
        if status.state == NativeAgentTaskStateV1::Cancelled {
            self.cancel_matching_workspace_movement(
                task_id,
                status.code.as_deref().unwrap_or("native_agent_cancelled"),
            );
        }
        self.persist_envelope(task_id, None)?;
        Ok(status)
    }

    pub(crate) fn mark_remote_cancel_delivery_uncertain(
        &mut self,
        task_id: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let status = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Remote native Agent task is unavailable.".into())
            })?;
            if task.state == NativeAgentTaskStateV1::Cancelled {
                task.code = Some("native_agent_cancel_delivery_uncertain".into());
            }
            task.clone()
        };
        if status.state == NativeAgentTaskStateV1::Cancelled {
            self.cancel_matching_workspace_movement(
                task_id,
                status.code.as_deref().unwrap_or("native_agent_cancelled"),
            );
        }
        self.persist_envelope(task_id, None)?;
        Ok(status)
    }

    pub(crate) fn fail_remote_delivery(
        &mut self,
        task_id: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let status = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Remote native Agent task is unavailable.".into())
            })?;
            if matches!(
                task.state,
                NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
            ) {
                // A receipt may be lost after the peer accepted invoke. Keep
                // this exact durable identity for fresh-session reconciliation;
                // it is not evidence that the Agent did not start.
                task.state = NativeAgentTaskStateV1::Interrupted;
                task.native_decisions.clear();
                task.code = Some("native_agent_reconciliation_required".into());
                log_native_lifecycle(
                    "remote_delivery_ambiguous",
                    task_id,
                    "state=Interrupted code=native_agent_reconciliation_required",
                );
            }
            task.clone()
        };
        self.persist_envelope(task_id, None)?;
        Ok(status)
    }

    /// The executing Host refused this invocation before admitting it (its
    /// rate budget, its Review capacity, or this device's authority), so it
    /// never started there: a definite non-start, unlike a lost receipt.
    /// Only for an invocation still waiting for its first status.
    pub(crate) fn fail_remote_undelivered(
        &mut self,
        task_id: &str,
        code: &str,
        reason: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let status = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Remote native Agent task is unavailable.".into())
            })?;
            if task.state == NativeAgentTaskStateV1::Queued
                && task.code.as_deref() == Some("remote_agent_queued")
            {
                task.state = NativeAgentTaskStateV1::Failed;
                task.native_decisions.clear();
                task.code = Some(code.into());
                task.result = Some(reason.into());
                log_native_lifecycle(
                    "remote_invocation_refused",
                    task_id,
                    &format!("state=Failed code={code}"),
                );
            }
            task.clone()
        };
        self.persist_envelope(task_id, None)?;
        Ok(status)
    }

    pub(crate) fn mark_outbound_workspace_delivery_failed(
        &mut self,
        movement_id: &str,
        receipt_ambiguous: bool,
    ) -> AppResult<NativeAgentWorkspaceMovementV1> {
        let code = if receipt_ambiguous {
            "native_agent_reconciliation_required"
        } else {
            "outbound_transfer_failed"
        };
        let task_id = {
            let movement = self
                .workspace_movements
                .get_mut(movement_id)
                .ok_or_else(|| {
                    AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
                })?;
            if !matches!(
                movement.status.state,
                NativeAgentWorkspaceMovementStateV1::Completed
                    | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                    | NativeAgentWorkspaceMovementStateV1::Cancelled
            ) {
                movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                movement.status.code = Some(code.into());
            }
            movement.status.task_id.clone()
        };
        if receipt_ambiguous {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            if let Some(task) = tasks.get_mut(&task_id) {
                if !matches!(
                    task.state,
                    NativeAgentTaskStateV1::Completed
                        | NativeAgentTaskStateV1::Failed
                        | NativeAgentTaskStateV1::Cancelled
                ) {
                    task.state = NativeAgentTaskStateV1::Interrupted;
                    task.native_decisions.clear();
                    task.code = Some(code.into());
                }
            }
        }
        self.persist_envelope(&task_id, None)?;
        self.movement_status(movement_id)
    }

    pub(crate) fn recovery_projection_for_bridge(
        &self,
        bridge_id: &str,
    ) -> AppResult<Option<NativeAgentRecoveryProjectionV1>> {
        let tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let mut unresolved = self
            .task_bridges
            .iter()
            .filter(|(_, bound_bridge)| bound_bridge.as_str() == bridge_id)
            .filter_map(|(task_id, _)| {
                let task = tasks.get(task_id)?;
                let movement = self
                    .workspace_movements
                    .values()
                    .find(|record| record.status.task_id == *task_id)
                    .map(|record| record.status.clone());
                let task_requires_resolution = task.state == NativeAgentTaskStateV1::Interrupted
                    && matches!(
                        task.code.as_deref(),
                        Some("native_agent_reconciliation_required")
                            | Some("native_agent_outcome_unknown")
                    );
                let movement_requires_resolution = movement.as_ref().is_some_and(|value| {
                    value.state == NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                        || (value.state == NativeAgentWorkspaceMovementStateV1::ReturningResult
                            && (value.code.is_none()
                                || value.code.as_deref() == Some("result_return_retry_required")))
                        || (value.state == NativeAgentWorkspaceMovementStateV1::Interrupted
                            && matches!(
                                value.code.as_deref(),
                                Some("native_agent_reconciliation_required")
                                    | Some("conflict_result_retention_required")
                                    | Some("conflict_result_retention_failed")
                                    | Some("result_apply_interrupted")
                                    | Some("native_agent_result_snapshot_recovery_failed")
                            ))
                });
                (task_requires_resolution || movement_requires_resolution).then(|| {
                    NativeAgentRecoveryProjectionV1 {
                        task: NativeAgentRecoveryTaskProjectionV1::from(task),
                        movement,
                        target_host_ref: self.remote_targets.get(task_id).cloned(),
                    }
                })
            })
            .collect::<Vec<_>>();
        unresolved.sort_by(|left, right| left.task.task_id.cmp(&right.task.task_id));
        Ok(unresolved.into_iter().next())
    }

    /// The requesting side's check before sending its user's decision: the
    /// task is this Bridge's, runs on `target_host_ref`, and its executing
    /// Host last reported this exact review pending, offering this decision.
    /// The executing Host decides again; this only refuses stale clicks.
    pub(crate) fn remote_decision_target(
        &self,
        bridge_id: &str,
        task_id: &str,
        review_ref: &str,
        content_digest: &str,
        decision: NativeDecisionV1,
    ) -> AppResult<String> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent task crossed its Bridge authority.");
        }
        let target = self
            .remote_targets
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("This task runs on this device.".into()))?;
        let tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let review = tasks
            .get(task_id)
            .filter(|status| status.state == NativeAgentTaskStateV1::Running)
            .and_then(|status| {
                status
                    .native_decisions
                    .iter()
                    .find(|review| review.review_ref == review_ref)
            })
            .filter(|review| review.state == NativeDecisionStateV1::Pending)
            .ok_or_else(|| {
                AppError::InvalidInput("Codex no longer waits for this decision.".into())
            })?;
        let offered = match decision {
            NativeDecisionV1::Accept => review.can_accept,
            NativeDecisionV1::Decline => review.can_decline,
        };
        if review.content_digest != content_digest || !offered {
            return invalid("What Codex asks changed; review it again.");
        }
        Ok(target)
    }

    pub(crate) fn bridge_remote_target(
        &self,
        bridge_id: &str,
        task_id: &str,
    ) -> AppResult<Option<String>> {
        if self.task_bridges.get(task_id).map(String::as_str) != Some(bridge_id) {
            return invalid("Native Agent task crossed its Bridge authority.");
        }
        Ok(self.remote_targets.get(task_id).cloned())
    }

    /// Explicitly revokes this Bridge's outer authority for one unresolved
    /// task. A locally owned native turn is interrupted best-effort after the
    /// terminal fact is installed; a remote target is returned to the caller
    /// for the existing authenticated cancellation path.
    pub(crate) fn stop_bridge_task_authority(
        &mut self,
        bridge_id: &str,
        task_id: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_bridges.get(task_id).map(String::as_str) != Some(bridge_id)
        {
            return invalid("Native Agent task crossed its Bridge authority.");
        }
        self.pending_reviews.remove(task_id);
        let status = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent task is unavailable.".into())
            })?;
            if !matches!(
                task.state,
                NativeAgentTaskStateV1::Completed
                    | NativeAgentTaskStateV1::Failed
                    | NativeAgentTaskStateV1::Cancelled
            ) {
                task.state = NativeAgentTaskStateV1::Cancelled;
                task.native_decisions.clear();
                task.code = Some("native_agent_cancel_requested".into());
            }
            task.clone()
        };
        if status.state == NativeAgentTaskStateV1::Cancelled {
            self.cancel_matching_workspace_movement(
                task_id,
                status.code.as_deref().unwrap_or("native_agent_cancelled"),
            );
        } else if matches!(
            status.state,
            NativeAgentTaskStateV1::Completed | NativeAgentTaskStateV1::Failed
        ) {
            // The Agent's terminal fact remains unchanged. The user is only
            // abandoning Pastey's unresolved movement authority.
            if let Some(movement) = self.workspace_movements.values_mut().find(|record| {
                record.status.task_id == task_id
                    && record.status.state == NativeAgentWorkspaceMovementStateV1::Interrupted
                    && matches!(
                        record.status.code.as_deref(),
                        Some("native_agent_reconciliation_required")
                            | Some("conflict_result_retention_required")
                            | Some("conflict_result_retention_failed")
                            | Some("result_apply_interrupted")
                            | Some("native_agent_result_snapshot_recovery_failed")
                    )
            }) {
                movement.status.state = NativeAgentWorkspaceMovementStateV1::Cancelled;
                movement.status.code = Some("native_agent_recovery_abandoned".into());
            }
        }
        self.persist_envelope(task_id, None)?;
        if self.unstarted_review_tasks.contains(task_id) {
            return Ok(status);
        }
        if let Ok(adapter) = self.adapter(&status.agent_id) {
            let _ = adapter.cancel(task_id);
        }
        Ok(status)
    }

    /// Records Bridge-scoped uncertainty after route loss without interrupting
    /// Host-local Codex sessions or live observers. Durable task, movement,
    /// result snapshot, retained conflict, and received workspace facts remain
    /// available for authenticated fresh-session reconciliation. Burn uses
    /// `purge_bridge_authority` below.
    pub(crate) fn revoke_bridge_session(&mut self, bridge_id: &str) -> AppResult<()> {
        // A Review is bound to the exact session that just ended; nothing it
        // held has started, so there is nothing to reconcile.
        self.end_bridge_reviews(bridge_id, admission::ADMISSION_REVOKED_CODE);
        let task_ids = self
            .task_bridges
            .iter()
            .filter_map(|(task_id, bound)| {
                (bound == bridge_id && !self.unstarted_review_tasks.contains(task_id))
                    .then_some(task_id.clone())
            })
            .collect::<Vec<_>>();
        let actively_observed_tasks = self
            .observed_tasks
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .clone();
        let mut completed_tasks = HashSet::new();
        {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            for task_id in &task_ids {
                if let Some(task) = tasks.get_mut(task_id) {
                    if task.state == NativeAgentTaskStateV1::Completed {
                        completed_tasks.insert(task_id.clone());
                    } else if matches!(
                        task.state,
                        NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
                    ) && !actively_observed_tasks.contains(task_id)
                    {
                        task.state = NativeAgentTaskStateV1::Interrupted;
                        task.native_decisions.clear();
                        task.code = Some("native_agent_reconciliation_required".into());
                        log_native_lifecycle(
                            "bridge_session_lost",
                            task_id,
                            "state=Interrupted code=native_agent_reconciliation_required",
                        );
                    }
                }
            }
        }

        let exact_completed_returns = self
            .durable_paths
            .as_ref()
            .map(|paths| {
                self.workspace_movements
                    .iter()
                    .filter_map(|(movement_id, record)| {
                        let (Some(snapshot), Some(identity)) =
                            (&record.result_snapshot, &record.result_identity)
                        else {
                            return None;
                        };
                        (completed_tasks.contains(&record.status.task_id)
                            && record.prepared_remote.is_some()
                            && record.apply_transaction.is_none()
                            && !matches!(
                                record.status.state,
                                NativeAgentWorkspaceMovementStateV1::Completed
                                    | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                                    | NativeAgentWorkspaceMovementStateV1::Failed
                                    | NativeAgentWorkspaceMovementStateV1::Cancelled
                            )
                            && validate_exact_app_owned_result(paths, snapshot, identity).is_ok())
                        .then(|| movement_id.clone())
                    })
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        let mut completed_results_waiting_for_snapshot = HashSet::new();
        let mut completed_result_recovery_failures = HashSet::new();
        for (movement_id, record) in &self.workspace_movements {
            if !completed_tasks.contains(&record.status.task_id)
                || record.prepared_remote.is_none()
                || record.apply_transaction.is_some()
                || record.apply_completed
                || matches!(
                    record.status.state,
                    NativeAgentWorkspaceMovementStateV1::Completed
                        | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                        | NativeAgentWorkspaceMovementStateV1::Failed
                        | NativeAgentWorkspaceMovementStateV1::Cancelled
                )
                || exact_completed_returns.contains(movement_id)
            {
                continue;
            }
            match (&record.result_snapshot, &record.result_identity) {
                (None, None) => {
                    let workspace = record
                        .task_workspace
                        .as_ref()
                        .or_else(|| self.task_workspaces.get(&record.status.task_id));
                    if workspace.is_some_and(|workspace| {
                        validate_workspace_transfer_fidelity(workspace).is_ok()
                    }) {
                        completed_results_waiting_for_snapshot.insert(movement_id.clone());
                    } else {
                        completed_result_recovery_failures.insert(movement_id.clone());
                    }
                }
                _ => {
                    // A partial or invalid identity cannot be replaced by a
                    // newly captured tree after the Agent has completed.
                    completed_result_recovery_failures.insert(movement_id.clone());
                }
            }
        }

        for (movement_id, record) in &mut self.workspace_movements {
            if !task_ids.contains(&record.status.task_id) {
                continue;
            }
            if exact_completed_returns.contains(movement_id) {
                record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
                record.status.code = Some("result_return_retry_required".into());
                continue;
            }
            if completed_results_waiting_for_snapshot.contains(movement_id) {
                // Completion is known. Leave a safe retained workspace for
                // the existing monitor, or restart recovery, to seal.
                record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
                record.status.code = Some("result_snapshot_capture_pending".into());
                continue;
            }
            if completed_result_recovery_failures.contains(movement_id) {
                record.status.state = NativeAgentWorkspaceMovementStateV1::Failed;
                record.status.code = Some("native_agent_result_snapshot_recovery_failed".into());
                continue;
            }
            if actively_observed_tasks.contains(&record.status.task_id) {
                continue;
            }
            if matches!(
                record.status.state,
                NativeAgentWorkspaceMovementStateV1::TransferringToAgent
                    | NativeAgentWorkspaceMovementStateV1::AgentRunning
                    | NativeAgentWorkspaceMovementStateV1::ReturningResult
                    | NativeAgentWorkspaceMovementStateV1::ApplyingResult
            ) {
                if record.source.is_some()
                    && completed_tasks.contains(&record.status.task_id)
                    && record.status.state == NativeAgentWorkspaceMovementStateV1::ReturningResult
                    && record.status.code.as_deref() == Some("result_return_retry_required")
                    && record.expected_return_digest.is_some()
                {
                    continue;
                }
                record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                record.status.code = Some(
                    if record.apply_transaction.is_some() {
                        "result_apply_interrupted"
                    } else {
                        "native_agent_reconciliation_required"
                    }
                    .into(),
                );
            }
        }

        // Bridge route loss revokes transport authority elsewhere in HostRuntime.
        // Codex sessions and live observers are Host-local Agent state and must
        // remain available; only explicit cancellation or Burn interrupts them.
        for task_id in task_ids {
            self.persist_envelope(&task_id, None)?;
        }
        Ok(())
    }

    /// Revokes only authority derived through one Bridge. The bridge marker is
    /// installed before native cancellation and cleanup so a late observer or
    /// remote fact cannot recreate a deleted envelope.
    /// After `purge_bridge_authority` for a Bridge that continues with a
    /// newly trusted peer (not a Burn): the Bridge may admit new work again.
    /// Nothing from before the purge comes back.
    pub(crate) fn readmit_bridge_after_fresh_trust(&mut self, bridge_id: &str) {
        self.revoked_bridges.remove(bridge_id);
    }

    pub(crate) fn purge_bridge_authority(&mut self, bridge_id: &str) -> AppResult<()> {
        self.revoked_bridges.insert(bridge_id.to_owned());
        self.pending_reviews
            .retain(|_, pending| pending.binding.bridge_id != bridge_id);
        self.movement_bindings
            .retain(|_, binding| binding.bridge_id != bridge_id);
        let task_ids = self
            .task_bridges
            .iter()
            .filter_map(|(task_id, bound)| (bound == bridge_id).then_some(task_id.clone()))
            .collect::<Vec<_>>();
        {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            for task_id in &task_ids {
                if let Some(task) = tasks.get_mut(task_id) {
                    task.state = NativeAgentTaskStateV1::Cancelled;
                    task.code = Some("bridge_authority_revoked".into());
                    task.native_decisions.clear();
                }
                self.task_requesters.remove(task_id);
            }
        }
        for adapter in &self.adapters {
            for task_id in &task_ids {
                // An adapter never saw a task that a Review did not start.
                if !self.unstarted_review_tasks.contains(task_id) {
                    adapter.release(task_id);
                }
            }
        }
        // The Bridge's peers' conversations end with it: their live native
        // state now, their durable records below and again on finalization.
        let mut conversation_keys = task_ids
            .iter()
            .filter_map(|task_id| self.task_conversations.remove(task_id))
            .collect::<HashSet<_>>();
        if let Some(paths) = self.durable_paths.as_ref() {
            conversation_keys.extend(crate::storage::delete_bridge_native_agent_conversations(
                paths, bridge_id,
            )?);
        }
        for adapter in &self.adapters {
            for key in &conversation_keys {
                adapter.release_conversation(key);
            }
        }
        let movement_ids = self
            .workspace_movements
            .iter()
            .filter_map(|(movement_id, record)| {
                task_ids
                    .contains(&record.status.task_id)
                    .then_some(movement_id.clone())
            })
            .collect::<Vec<_>>();
        for movement_id in &movement_ids {
            if let Some(record) = self.workspace_movements.get_mut(movement_id) {
                if let Err(error) = recover_workspace_apply_record(movement_id, record) {
                    // Burn has already revoked Bridge authority. Preserve an
                    // unprovable canonical tree and its siblings, then remove
                    // the envelope so startup cannot restore that authority.
                    crate::logging::write_error_line(&format!(
                        "Native Agent Burn left an unresolved workspace apply journal: {}",
                        error.message()
                    ));
                }
            }
        }
        if let Some(paths) = self.durable_paths.as_ref() {
            for movement_id in &movement_ids {
                if let Some(record) = self.workspace_movements.get(movement_id) {
                    if let Some(conflict) =
                        crate::storage::get_native_agent_conflict(paths, movement_id)?
                    {
                        delete_exact_retained_conflict_container(paths, &conflict)?;
                        crate::storage::delete_native_agent_conflict(paths, movement_id)?;
                    }
                    if let Some(snapshot) = record.result_snapshot.as_ref() {
                        delete_exact_app_owned_result(paths, snapshot)?;
                    }
                    if record.prepared_remote.is_some() {
                        if let Some(workspace) = record.task_workspace.as_ref() {
                            crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
                        }
                    }
                }
            }
            for task_id in &task_ids {
                crate::storage::delete_native_agent_envelope(paths, task_id)?;
            }
        }
        self.workspace_movements
            .retain(|_, record| !task_ids.contains(&record.status.task_id));
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain(|task_id, _| !task_ids.contains(task_id));
        }
        for task_id in task_ids {
            self.remote_targets.remove(&task_id);
            self.task_workspaces.remove(&task_id);
            self.task_digests.remove(&task_id);
            self.task_exclusivity.remove(&task_id);
            self.task_input_digests.remove(&task_id);
            self.unstarted_review_tasks.remove(&task_id);
            self.task_bridges.remove(&task_id);
        }
        Ok(())
    }

    /// Cancellation received over a Bridge. Only the Bridge that owns the
    /// task may cancel it. Another Bridge's task and a Local task are refused
    /// untouched, with the same answer as an unknown task.
    pub(crate) fn cancel_bridge_task(
        &mut self,
        bridge_id: &str,
        task_id: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.revoked_bridges.contains(bridge_id)
            || self.task_owner(task_id)? != Some(TaskOwnerV1::Bridge(bridge_id.to_owned()))
        {
            return invalid("Native Agent task is unavailable.");
        }
        self.cancel_task(task_id)
    }

    pub(crate) fn cancel_task(&mut self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        // Cancelling a task held for Review ends the Review; it never started.
        self.pending_reviews.remove(task_id);
        let (status, native_execution_needs_cancellation) = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent task is unavailable.".into())
            })?;
            // An uncertain turn, also one restored after restart for
            // reconciliation, is resolved only by this explicit cancellation.
            let uncertain_active_turn = task.state == NativeAgentTaskStateV1::Interrupted
                && matches!(
                    task.code.as_deref(),
                    Some("native_agent_outcome_unknown")
                        | Some("native_agent_reconciliation_required")
                );
            if !matches!(
                task.state,
                NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
            ) && !uncertain_active_turn
            {
                return Ok(task.clone());
            }
            // Revoke Pastey's execution/result authority before attempting a
            // fallible native interrupt. This is not proof the process ended.
            let native_execution_needs_cancellation = task.state != NativeAgentTaskStateV1::Queued;
            task.state = NativeAgentTaskStateV1::Cancelled;
            task.code = Some("native_agent_cancel_requested".into());
            // No outstanding review may be answered once stopping began.
            task.native_decisions.clear();
            (task.clone(), native_execution_needs_cancellation)
        };
        self.cancel_matching_workspace_movement(
            task_id,
            status.code.as_deref().unwrap_or("native_agent_cancelled"),
        );
        self.persist_envelope(task_id, None)?;
        if !native_execution_needs_cancellation {
            return Ok(status);
        }
        // The adapter owns how its native execution stops; Pastey's authority
        // was revoked above whatever this returns.
        let native_stop = self
            .adapter(&status.agent_id)
            .and_then(|adapter| adapter.cancel(task_id));
        match native_stop {
            Ok(()) => Ok(status),
            Err(_) => {
                let status = {
                    let mut tasks = self.tasks.lock().map_err(|_| {
                        AppError::InvalidInput("Native Agent task store is unavailable.".into())
                    })?;
                    let task = tasks.get_mut(task_id).ok_or_else(|| {
                        AppError::InvalidInput("Native Agent task is unavailable.".into())
                    })?;
                    // The task remains cancelled even when delivery of its native
                    // interrupt is uncertain; workspace occupancy remains until
                    // the observer actually exits.
                    task.code = Some("native_agent_cancel_delivery_uncertain".into());
                    task.clone()
                };
                self.cancel_matching_workspace_movement(
                    task_id,
                    status.code.as_deref().unwrap_or("native_agent_cancelled"),
                );
                self.persist_envelope(task_id, None)?;
                Ok(status)
            }
        }
    }

    pub(crate) fn shutdown(&mut self) {
        // The registry owns adapter lifecycles: each registered adapter is
        // shut down exactly once. `codex` is only movement's typed alias of
        // the registered Codex adapter; when Codex is not registered it can
        // never have started a session.
        for adapter in &self.adapters {
            adapter.shutdown();
        }
        self.pending_reviews.clear();
        self.active_exclusivity
            .lock()
            .ok()
            .map(|mut active| active.clear());
        self.observed_tasks
            .lock()
            .ok()
            .map(|mut observed| observed.clear());
        self.task_workspaces.clear();
    }
}

/// Startup/finalization counterpart to the process-local revocation above.
/// It removes only app-owned Native Agent material whose durable envelope is
/// explicitly correlated to the burned Bridge.
pub(crate) fn purge_durable_bridge_state(
    paths: &crate::storage::AppPaths,
    bridge_id: &str,
) -> AppResult<()> {
    crate::storage::delete_bridge_native_agent_conversations(paths, bridge_id)?;
    for stored in crate::storage::list_native_agent_envelopes(paths)? {
        let mut persisted: PersistedNativeAgentEnvelopeV1 =
            serde_json::from_str(&stored.record_json).map_err(AppError::from)?;
        let bound_bridge = persisted.bridge_id.as_deref().or_else(|| {
            persisted
                .movement
                .as_ref()
                .and_then(|movement| movement.bridge_id.as_deref())
        });
        if bound_bridge != Some(bridge_id) {
            continue;
        }
        if let Some(movement) = persisted.movement.as_mut() {
            let movement_id = movement.status.movement_id.clone();
            if let Err(error) = recover_workspace_apply_record(&movement_id, movement) {
                // The durable Burn tombstone is authoritative. Unknown
                // canonical, stage, and backup trees remain untouched.
                crate::logging::write_error_line(&format!(
                    "Native Agent burned-room cleanup left an unresolved workspace apply journal: {}",
                    error.message()
                ));
            }
            if let Some(conflict) =
                crate::storage::get_native_agent_conflict(paths, &movement.status.movement_id)?
            {
                delete_exact_retained_conflict_container(paths, &conflict)?;
                crate::storage::delete_native_agent_conflict(paths, &movement.status.movement_id)?;
            }
            if let Some(snapshot) = movement.result_snapshot.as_ref() {
                delete_exact_app_owned_result(paths, snapshot)?;
            }
            if movement.prepared_remote.is_some() {
                if let Some(workspace) = movement.task_workspace.as_ref() {
                    crate::regular_file_set_transfer::cleanup_materialized_tree(workspace);
                }
            }
        }
        crate::storage::delete_native_agent_envelope(paths, &stored.task_id)?;
    }
    Ok(())
}

/// RegularFileSet deliberately represents only regular file bytes.  Native
/// Agent workspace movement must therefore reject, before Review or Return,
/// every workspace feature that that representation would silently discard.
/// Local native tasks never pass through this gate: Codex sees their original
/// workspace and keeps its own native workspace semantics.
fn validate_workspace_transfer_fidelity(root: &Path) -> AppResult<()> {
    let root_metadata = fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return invalid("Native Agent workspace Transfer requires a real directory.");
    }
    let canonical_root = root.canonicalize()?;
    let mut pending = vec![canonical_root.clone()];
    let mut case_folded_selectors = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        let metadata = fs::symlink_metadata(&directory)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !directory.starts_with(&canonical_root)
        {
            return invalid("Native Agent workspace Transfer contains an unsafe directory.");
        }
        let entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        if entries.is_empty() {
            return invalid(
                "Native Agent workspace Transfer is blocked because empty directories are not portable.",
            );
        }
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return invalid(
                    "Native Agent workspace Transfer is blocked because symlinks are not portable.",
                );
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return invalid(
                    "Native Agent workspace Transfer is blocked because special files are not portable.",
                );
            }
            #[cfg(unix)]
            if std::os::unix::fs::MetadataExt::mode(&metadata) & 0o111 != 0 {
                return invalid(
                    "Native Agent workspace Transfer is blocked because executable file modes are not portable.",
                );
            }
            let relative = path.strip_prefix(&canonical_root).map_err(|_| {
                AppError::InvalidInput("Native Agent workspace Transfer escaped its root.".into())
            })?;
            let selector = relative
                .components()
                .map(|component| {
                    component.as_os_str().to_str().ok_or_else(|| {
                        AppError::InvalidInput(
                            "Native Agent workspace Transfer has a non-portable selector.".into(),
                        )
                    })
                })
                .collect::<AppResult<Vec<_>>>()?
                .join("/");
            crate::safe_file_identity::validate_managed_selector(&selector)?;
            validate_windows_portable_selector(&selector)?;
            if !case_folded_selectors.insert(selector.to_lowercase()) {
                return invalid(
                    "Native Agent workspace Transfer is blocked by case-colliding selectors.",
                );
            }
        }
    }
    Ok(())
}

fn validate_windows_portable_selector(selector: &str) -> AppResult<()> {
    for component in selector.split('/') {
        if component.is_empty()
            || component.ends_with('.')
            || component.ends_with(' ')
            || component.chars().any(|character| {
                character.is_control()
                    || matches!(character, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
            })
        {
            return invalid("Native Agent workspace Transfer has a non-portable selector.");
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return invalid("Native Agent workspace Transfer has a Windows-reserved selector.");
        }
    }
    Ok(())
}

fn apply_transaction_paths(
    source_workspace: &Path,
    movement_id: &str,
) -> AppResult<(PathBuf, PathBuf)> {
    let parent = source_workspace.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
    })?;
    let token = blake3::hash(movement_id.as_bytes()).to_hex().to_string();
    Ok((
        parent.join(format!(".pastey-agent-apply-{token}")),
        parent.join(format!(".pastey-agent-backup-{token}")),
    ))
}

fn identity_at(
    path: &Path,
) -> AppResult<Option<crate::safe_file_identity::RegularFileSetIdentity>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return invalid("Native Agent apply transaction directory is not a real directory.");
    }
    crate::safe_file_identity::capture_regular_file_set_identity(
        path,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )
    .map(Some)
}

fn stage_exact_workspace_result(
    stage: &Path,
    returned_workspace: &Path,
    returned_identity: &crate::safe_file_identity::RegularFileSetIdentity,
) -> AppResult<()> {
    if let Some(existing) = identity_at(stage)? {
        if same_logical_file_set(&existing, returned_identity) {
            return Ok(());
        }
        return invalid("Native Agent apply staging path belongs to an unexpected tree.");
    }
    fs::create_dir(stage)?;
    for (selector, identity) in &returned_identity.files {
        let bytes = crate::safe_file_identity::read_source_if_identity_matches(
            &returned_workspace.join(selector),
            returned_workspace,
            identity,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        let destination = stage.join(selector);
        let directory = destination.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent result selector is unavailable.".into())
        })?;
        fs::create_dir_all(directory)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_all()?;
    }
    let staged = crate::safe_file_identity::capture_regular_file_set_identity(
        stage,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if !same_logical_file_set(&staged, returned_identity) {
        return invalid("Native Agent staged result changed before apply.");
    }
    sync_directory(stage)?;
    Ok(())
}

fn sync_directory(path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn mark_workspace_apply_committed(
    record: &mut WorkspaceMovementRecordV1,
    result_identity: &crate::safe_file_identity::RegularFileSetIdentity,
) {
    record.result_identity = Some(result_identity.clone());
    record.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
    record.status.code = None;
    record.apply_completed = true;
    if let Some(transaction) = record.apply_transaction.as_mut() {
        transaction.phase = WorkspaceApplyPhaseV1::Committed;
    }
}

fn finish_committed_apply_cleanup(
    movement_id: &str,
    record: &mut WorkspaceMovementRecordV1,
) -> AppResult<()> {
    let Some(transaction) = record.apply_transaction.as_ref() else {
        return Ok(());
    };
    let source = record.source.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Native Agent apply source is unavailable.".into())
    })?;
    let result_identity = transaction.result_identity.clone();
    let (stage, backup) = apply_transaction_paths(&source.workspace, movement_id)?;
    let installed = identity_at(&source.workspace)?.ok_or_else(|| {
        AppError::InvalidInput(
            "Native Agent canonical workspace is unavailable after apply.".into(),
        )
    })?;
    if !same_logical_file_set(&installed, &result_identity) {
        return invalid("Native Agent committed result does not match its durable apply record.");
    }
    if let Some(original) = identity_at(&backup)? {
        if original != source.baseline {
            return invalid("Native Agent apply backup does not match the approved original.");
        }
        fs::remove_dir_all(&backup)?;
    }
    if identity_at(&stage)?.is_some() {
        fs::remove_dir_all(&stage)?;
    }
    sync_directory(source.workspace.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
    })?)?;
    record.apply_transaction = None;
    Ok(())
}

/// Reconciles the filesystem against its durable apply intent. A staged exact
/// result completes the authorized apply; when staging did not finish, the
/// original is restored to its canonical path and the exact result remains
/// retryable. Every backup path is derived from the movement identity recorded
/// beside it in the durable envelope.
fn recover_workspace_apply_record(
    movement_id: &str,
    record: &mut WorkspaceMovementRecordV1,
) -> AppResult<()> {
    let Some(transaction) = record.apply_transaction.as_ref().cloned() else {
        return Ok(());
    };
    let source = record.source.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Native Agent apply source is unavailable.".into())
    })?;
    let source_path = source.workspace.clone();
    let baseline = source.baseline.clone();
    let result_identity = transaction.result_identity;
    let (stage, backup) = apply_transaction_paths(&source_path, movement_id)?;
    let current = identity_at(&source_path)?;
    let staged = identity_at(&stage)?;
    let backed_up = identity_at(&backup)?;
    let current_is_result = current
        .as_ref()
        .is_some_and(|identity| same_logical_file_set(identity, &result_identity));
    let stage_is_result = staged
        .as_ref()
        .is_some_and(|identity| same_logical_file_set(identity, &result_identity));
    let backup_is_original = backed_up
        .as_ref()
        .is_some_and(|identity| *identity == baseline);
    let current_is_original = current
        .as_ref()
        .is_some_and(|identity| *identity == baseline);

    if current_is_result {
        mark_workspace_apply_committed(record, &result_identity);
        // Cleanup may fail after the canonical result and durable completion
        // agree. Keep the journal so startup can retry only this cleanup.
        let _ = finish_committed_apply_cleanup(movement_id, record);
        return Ok(());
    }

    if current_is_original && stage_is_result {
        if backup_is_original {
            fs::remove_dir_all(&backup)?;
        } else if backed_up.is_some() {
            return invalid("Native Agent apply backup has an unexpected identity.");
        }
        fs::rename(&source_path, &backup)?;
        sync_directory(source_path.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
        })?)?;
        fs::rename(&stage, &source_path)?;
        sync_directory(source_path.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
        })?)?;
        let installed = identity_at(&source_path)?.ok_or_else(|| {
            AppError::InvalidInput(
                "Native Agent result workspace is unavailable after apply.".into(),
            )
        })?;
        if !same_logical_file_set(&installed, &result_identity) {
            return invalid(
                "Native Agent recovered apply result does not match its durable identity.",
            );
        }
        mark_workspace_apply_committed(record, &result_identity);
        let _ = finish_committed_apply_cleanup(movement_id, record);
        return Ok(());
    }

    if current.is_none() && backup_is_original && stage_is_result {
        fs::rename(&stage, &source_path)?;
        sync_directory(source_path.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
        })?)?;
        mark_workspace_apply_committed(record, &result_identity);
        let _ = finish_committed_apply_cleanup(movement_id, record);
        return Ok(());
    }

    if current.is_none() && backup_is_original {
        fs::rename(&backup, &source_path)?;
        sync_directory(source_path.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
        })?)?;
        if staged.is_some() {
            let _ = fs::remove_dir_all(&stage);
        }
        record.apply_transaction = None;
        record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
        record.status.code = Some("result_apply_interrupted".into());
        record.apply_completed = false;
        record.result_identity = Some(result_identity);
        return Ok(());
    }

    if current_is_original && !stage_is_result {
        if staged.is_some() {
            fs::remove_dir_all(&stage)?;
        }
        if backup_is_original {
            fs::remove_dir_all(&backup)?;
        } else if backed_up.is_some() {
            return invalid("Native Agent apply backup has an unexpected identity.");
        }
        record.apply_transaction = None;
        record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
        record.status.code = Some("result_apply_interrupted".into());
        record.apply_completed = false;
        record.result_identity = Some(result_identity);
        return Ok(());
    }

    if current.is_none() && backed_up.is_none() && stage_is_result {
        // The original was already moved by an earlier transaction attempt,
        // and the exact authorized result remains fully staged.
        fs::rename(&stage, &source_path)?;
        sync_directory(source_path.parent().ok_or_else(|| {
            AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
        })?)?;
        mark_workspace_apply_committed(record, &result_identity);
        let _ = finish_committed_apply_cleanup(movement_id, record);
        return Ok(());
    }

    // Do not overwrite an unexpected canonical tree or discard a backup that
    // cannot be proved to be the approved original. The persisted journal keeps
    // the hidden sibling tracked and the failure remains recoverable.
    Err(AppError::InvalidInput(
        "Native Agent apply transaction cannot prove a safe canonical workspace state.".into(),
    ))
}

fn same_logical_file_set(
    left: &crate::safe_file_identity::RegularFileSetIdentity,
    right: &crate::safe_file_identity::RegularFileSetIdentity,
) -> bool {
    left.digest == right.digest
        && left.byte_count == right.byte_count
        && left
            .files
            .iter()
            .map(|(selector, identity)| (selector, &identity.digest, identity.byte_count))
            .eq(right
                .files
                .iter()
                .map(|(selector, identity)| (selector, &identity.digest, identity.byte_count)))
}

fn retain_conflicted_workspace(
    paths: &crate::storage::AppPaths,
    movement_id: &str,
    task_id: &str,
    returned_workspace: &Path,
) -> AppResult<PathBuf> {
    validate_movement_id(movement_id)?;
    validate_task_id(task_id)?;
    validate_workspace_transfer_fidelity(returned_workspace)?;
    let identity = crate::safe_file_identity::capture_regular_file_set_identity(
        returned_workspace,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if let Some(existing) = crate::storage::get_native_agent_conflict(paths, movement_id)? {
        if existing.task_id != task_id
            || existing.result_digest != identity.digest
            || existing.result_byte_count != identity.byte_count
        {
            return invalid(
                "Native Agent conflict result correlation is already bound differently.",
            );
        }
        validate_retained_conflict(paths, &existing)?;
        return Ok(existing.retained_tree);
    }
    let unique = Uuid::new_v4().to_string();
    let retained = crate::safe_file_identity::create_private_tree_root(
        &paths.app_data_dir,
        "native-agent-conflicts",
        &unique,
    )?;
    let result = (|| {
        for (selector, source_identity) in &identity.files {
            let bytes = crate::safe_file_identity::read_source_if_identity_matches(
                &returned_workspace.join(selector),
                returned_workspace,
                source_identity,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )?;
            let mut output =
                crate::safe_file_identity::create_private_regular_file(&retained, selector)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
        }
        let observed = crate::safe_file_identity::capture_regular_file_set_identity(
            &retained,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if !same_logical_file_set(&observed, &identity) {
            return invalid("Native Agent conflict result did not survive durable retention.");
        }
        crate::storage::save_native_agent_conflict(
            paths,
            &crate::storage::StoredNativeAgentConflict {
                movement_id: movement_id.into(),
                task_id: task_id.into(),
                retained_tree: retained.clone(),
                result_digest: identity.digest.clone(),
                result_byte_count: identity.byte_count,
                created_at: crate::storage::now_ts(),
            },
        )?;
        Ok(())
    })();
    if result.is_err() {
        if let Some(root) = retained.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
    result.map(|()| retained)
}

fn conflict_root(paths: &crate::storage::AppPaths) -> PathBuf {
    paths.app_data_dir.join("native-agent-conflicts")
}

/// Resolves a retained result only after proving that it is still an
/// app-owned conflict tree with the exact durable identity. The path never
/// enters a renderer-safe DTO.
fn validate_retained_conflict(
    paths: &crate::storage::AppPaths,
    conflict: &crate::storage::StoredNativeAgentConflict,
) -> AppResult<PathBuf> {
    let root = fs::canonicalize(conflict_root(paths))
        .map_err(|_| AppError::InvalidInput("Native Agent conflict root is unavailable.".into()))?;
    let retained = fs::canonicalize(&conflict.retained_tree).map_err(|_| {
        AppError::InvalidInput("Native Agent retained conflict result is unavailable.".into())
    })?;
    if !retained.starts_with(&root)
        || retained.file_name().and_then(|name| name.to_str()) != Some("tree")
    {
        return invalid("Native Agent retained conflict result is outside its private root.");
    }
    let identity = crate::safe_file_identity::capture_regular_file_set_identity(
        &retained,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if identity.digest != conflict.result_digest
        || identity.byte_count != conflict.result_byte_count
    {
        return invalid("Native Agent retained conflict result no longer matches its receipt.");
    }
    Ok(retained)
}

/// Removes only the unique app-owned container that holds one exact retained
/// conflict tree. A missing tree is an expected restart case after the
/// filesystem deletion completed but before its SQLite receipt was removed.
fn delete_exact_retained_conflict_container(
    paths: &crate::storage::AppPaths,
    conflict: &crate::storage::StoredNativeAgentConflict,
) -> AppResult<()> {
    if !conflict.retained_tree.exists() {
        return Ok(());
    }
    let retained = validate_retained_conflict(paths, conflict)?;
    let container = retained.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent retained conflict container is unavailable.".into())
    })?;
    let root = fs::canonicalize(conflict_root(paths))
        .map_err(|_| AppError::InvalidInput("Native Agent conflict root is unavailable.".into()))?;
    let container = fs::canonicalize(container).map_err(|_| {
        AppError::InvalidInput("Native Agent retained conflict container is unavailable.".into())
    })?;
    if !container.starts_with(&root) || container.parent() != Some(root.as_path()) {
        return invalid("Native Agent retained conflict container is outside its private root.");
    }
    fs::remove_dir_all(&container)?;
    Ok(())
}

fn delete_exact_app_owned_result(
    paths: &crate::storage::AppPaths,
    snapshot: &Path,
) -> AppResult<()> {
    if !snapshot.exists() {
        return Ok(());
    }
    let root = fs::canonicalize(paths.app_data_dir.join("native-agent-results"))
        .map_err(|_| AppError::InvalidInput("Native Agent result root is unavailable.".into()))?;
    let snapshot = fs::canonicalize(snapshot).map_err(|_| {
        AppError::InvalidInput("Native Agent retained result is unavailable.".into())
    })?;
    let container = snapshot.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent retained result container is unavailable.".into())
    })?;
    if !snapshot.starts_with(&root)
        || !container.starts_with(&root)
        || container.parent() != Some(root.as_path())
    {
        return invalid("Native Agent retained result is outside its private root.");
    }
    fs::remove_dir_all(container)?;
    Ok(())
}

fn validate_exact_app_owned_result(
    paths: &crate::storage::AppPaths,
    snapshot: &Path,
    expected: &crate::safe_file_identity::RegularFileSetIdentity,
) -> AppResult<()> {
    let root = fs::canonicalize(paths.app_data_dir.join("native-agent-results"))
        .map_err(|_| AppError::InvalidInput("Native Agent result root is unavailable.".into()))?;
    let snapshot = fs::canonicalize(snapshot).map_err(|_| {
        AppError::InvalidInput("Native Agent retained result is unavailable.".into())
    })?;
    let container = snapshot.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent retained result container is unavailable.".into())
    })?;
    if !snapshot.starts_with(&root)
        || !container.starts_with(&root)
        || container.parent() != Some(root.as_path())
    {
        return invalid("Native Agent retained result is outside its private root.");
    }
    let observed = crate::safe_file_identity::capture_regular_file_set_identity(
        &snapshot,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if !same_logical_file_set(&observed, expected) {
        return invalid("Native Agent retained result no longer matches its durable identity.");
    }
    Ok(())
}

fn snapshot_exact_workspace_result(
    paths: &crate::storage::AppPaths,
    movement_id: &str,
    workspace: &Path,
) -> AppResult<(PathBuf, crate::safe_file_identity::RegularFileSetIdentity)> {
    validate_movement_id(movement_id)?;
    validate_workspace_transfer_fidelity(workspace)?;
    let identity = crate::safe_file_identity::capture_regular_file_set_identity(
        workspace,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    let snapshot = crate::safe_file_identity::create_private_tree_root(
        &paths.app_data_dir,
        "native-agent-results",
        &Uuid::new_v4().to_string(),
    )?;
    let result = (|| {
        for (selector, source_identity) in &identity.files {
            let bytes = crate::safe_file_identity::read_source_if_identity_matches(
                &workspace.join(selector),
                workspace,
                source_identity,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )?;
            let mut output =
                crate::safe_file_identity::create_private_regular_file(&snapshot, selector)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
        }
        let observed = crate::safe_file_identity::capture_regular_file_set_identity(
            &snapshot,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if !same_logical_file_set(&observed, &identity) {
            return invalid("Native Agent result did not survive durable snapshot.");
        }
        Ok(())
    })();
    if result.is_err() {
        if let Some(root) = snapshot.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
    result.map(|()| (snapshot, identity))
}

/// Finalizes an encrypted workspace Transfer into a Host-private task tree or
/// a retained returned-result tree. ManagedObject binding is retained for the
/// real cross-Host object movement; the Agent is only started after the
/// outbound receipt is exact.
pub(crate) fn register_workspace_transfer_landing(
    runtime: &crate::host_runtime::HostRuntime,
    metadata: &NativeAgentWorkspaceTransferV1,
    package_path: PathBuf,
    now: i64,
) -> AppResult<NativeAgentWorkspaceMovementV1> {
    let validation = runtime
        .native_agents
        .lock()
        .validate_workspace_transfer(metadata, runtime.local_host_ref.as_str());
    if let Err(error) = validation {
        crate::regular_file_set_transfer::cleanup_received_package(&package_path);
        return Err(error);
    }
    if metadata.phase == NativeAgentWorkspaceTransferPhaseV1::Return
        && runtime
            .native_agents
            .lock()
            .return_is_already_applied(metadata)
    {
        crate::regular_file_set_transfer::cleanup_received_package(&package_path);
        return runtime
            .native_agents
            .lock()
            .movement_status(&metadata.movement_id);
    }
    if metadata.phase == NativeAgentWorkspaceTransferPhaseV1::Return
        && runtime
            .native_agents
            .lock()
            .return_is_already_conflicted(metadata, &runtime.paths)
    {
        crate::regular_file_set_transfer::cleanup_received_package(&package_path);
        return runtime
            .native_agents
            .lock()
            .movement_status(&metadata.movement_id);
    }
    let materialized = crate::regular_file_set_transfer::materialize_package(
        &package_path,
        &runtime.paths.temp_dir,
        &metadata.content_digest,
        metadata.logical_byte_count,
    );
    crate::regular_file_set_transfer::cleanup_received_package(&package_path);
    let tree = materialized?;
    let scope_root = tree.parent().map(Path::to_path_buf).ok_or_else(|| {
        AppError::InvalidInput("Native Agent workspace receipt root is unavailable.".into())
    })?;
    let acquisition = runtime.managed_objects.lock().bind_transferred_revision(
        crate::managed_objects::HostArtifactAcquisition {
            kind: crate::managed_objects::ManagedObjectAcquisitionKind::TransferReceipt,
            source_ref: format!(
                "native-agent-workspace:{}:{:?}",
                metadata.movement_id, metadata.phase
            ),
            bridge_id: Some(metadata.bridge_id.clone()),
            path: tree.clone(),
            scope_root,
            display_name: "native-agent-workspace".into(),
            media_type: "application/octet-stream".into(),
            expires_at: now + 60 * 60,
            app_owned_temporary: true,
        },
        metadata.object.logical_object_id.clone(),
        metadata.object.revision,
        metadata.content_digest.clone(),
        now,
    );
    let acquisition = match acquisition {
        Ok(acquisition) => acquisition,
        Err(error) => {
            crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
            return Err(error);
        }
    };
    if acquisition.object.size_bytes != metadata.logical_byte_count
        || acquisition.object.host_ref.as_str() != metadata.destination_host_ref
        || acquisition.object.representation
            != crate::managed_objects::ManagedArtifactRepresentationV1::RegularFileSet
    {
        runtime.managed_objects.lock().discard_binding(&acquisition);
        crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
        return invalid("Native Agent workspace Transfer receipt content is invalid.");
    }
    let outcome = match metadata.phase {
        NativeAgentWorkspaceTransferPhaseV1::Outbound => runtime
            .native_agents
            .lock()
            .start_received_workspace_task(&metadata.movement_id, &tree)
            .map(|_| None),
        NativeAgentWorkspaceTransferPhaseV1::Return => runtime
            .native_agents
            .lock()
            .apply_received_workspace_return(&metadata.movement_id, &tree)
            .map(Some),
    };
    // The Host-local managed binding is only needed while validating this
    // receipt. Native Agent execution owns the app-materialized workspace
    // directly, so retaining the binding would add no authority or recovery.
    runtime.managed_objects.lock().discard_binding(&acquisition);
    match outcome {
        Ok(Some(status)) if status.code.as_deref() == Some("conflict_result_retention_pending") => {
            if let Err(error) = retain_conflicted_workspace(
                &runtime.paths,
                &metadata.movement_id,
                &metadata.task_id,
                &tree,
            ) {
                runtime.native_agents.lock().interrupt_workspace_movement(
                    &metadata.movement_id,
                    "conflict_result_retention_failed",
                );
                crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
                return Err(error);
            }
            crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
            runtime
                .native_agents
                .lock()
                .finalize_conflict_recovery(&metadata.movement_id, &runtime.paths)
        }
        Ok(_) => {
            if metadata.phase == NativeAgentWorkspaceTransferPhaseV1::Return {
                crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
            }
            runtime
                .native_agents
                .lock()
                .movement_status(&metadata.movement_id)
        }
        Err(error) => {
            crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
            Err(error)
        }
    }
}

/// Watches the native lifecycle after an outbound workspace has
/// landed. Native completion is reported first; only then does Pastey scan the
/// task tree and use the existing encrypted Transfer path for the result.
pub(crate) async fn monitor_received_workspace_task(
    runtime: Arc<crate::host_runtime::HostRuntime>,
    room_id: String,
    peer_session_id: String,
    movement_id: String,
) {
    let mut last: Option<NativeAgentTaskStatusV1> = None;
    loop {
        let task = {
            let mut service = runtime.native_agents.lock();
            // A landed workspace's Review ends by Accept, Deny or its bounded
            // expiry, which this existing loop also evaluates.
            service.expire_invocation_reviews(crate::storage::now_ts());
            let movement = match service.movement_status(&movement_id) {
                Ok(value) => value,
                Err(_) => return,
            };
            service.task_status(&movement.task_id).ok()
        };
        let Some(task) = task else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        if last.as_ref() != Some(&task) {
            if let Ok(context) = crate::room_control::room_control_session_context_for_peer(
                &runtime,
                &room_id,
                &peer_session_id,
            ) {
                if let Ok(payload) = serde_json::to_value(NativeAgentStatusV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: task.task_id.clone(),
                    executing_host_ref: runtime.local_host_ref.as_str().into(),
                    status: task.clone(),
                }) {
                    // Offered again only while the requester refuses it for
                    // its rate budget, which proves it never arrived.
                    let _ = crate::room_control::send_room_control_event_with_backpressure(
                        runtime.clone(),
                        &room_id,
                        || {
                            crate::room_control::native_agent_event(
                                "native_agent.status",
                                payload.clone(),
                                &context,
                            )
                        },
                        Some(crate::room_control::selected_peer_route(
                            &room_id,
                            &peer_session_id,
                        )),
                        Duration::from_secs(30),
                    )
                    .await;
                }
            }
            last = Some(task.clone());
        }
        match task.state {
            NativeAgentTaskStateV1::Running | NativeAgentTaskStateV1::Queued => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            NativeAgentTaskStateV1::Completed => {
                if send_workspace_result(runtime.clone(), &room_id, &movement_id)
                    .await
                    .is_err()
                {
                    let consequence_fact = {
                        let mut service = runtime.native_agents.lock();
                        service.mark_result_return_pending(&movement_id);
                        let failed = service.movement_status(&movement_id).is_ok_and(|movement| {
                            movement.state == NativeAgentWorkspaceMovementStateV1::Failed
                                && movement.code.as_deref()
                                    == Some("native_agent_result_snapshot_recovery_failed")
                        });
                        let source_host_ref = service
                            .workspace_movements
                            .get(&movement_id)
                            .and_then(|record| record.prepared_remote.as_ref())
                            .map(|prepared| prepared.source_host_ref.clone())
                            .unwrap_or_default();
                        failed.then(|| {
                            service.reconciliation_fact(
                                &room_id,
                                &task.task_id,
                                Some(&movement_id),
                                runtime.local_host_ref.as_str(),
                                &source_host_ref,
                            )
                        })
                    };
                    if let Some(Ok(fact)) = consequence_fact {
                        if let Ok(context) =
                            crate::room_control::room_control_session_context_for_peer(
                                &runtime,
                                &room_id,
                                &peer_session_id,
                            )
                        {
                            if let Ok(event) = serde_json::to_value(fact)
                                .map_err(AppError::from)
                                .and_then(|payload| {
                                    crate::room_control::native_agent_event(
                                        "native_agent.reconciliation",
                                        payload,
                                        &context,
                                    )
                                })
                            {
                                let _ = crate::room_control::send_room_control_event(
                                    runtime.clone(),
                                    &room_id,
                                    event,
                                    Some(crate::room_control::selected_peer_route(
                                        &room_id,
                                        &peer_session_id,
                                    )),
                                )
                                .await;
                            }
                        }
                    }
                }
                return;
            }
            NativeAgentTaskStateV1::Failed | NativeAgentTaskStateV1::Cancelled => {
                if runtime
                    .native_agents
                    .lock()
                    .cleanup_terminal_remote_workspace(&movement_id)
                    .unwrap_or(false)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            NativeAgentTaskStateV1::Interrupted => return,
        }
    }
}

async fn send_workspace_result(
    runtime: Arc<crate::host_runtime::HostRuntime>,
    room_id: &str,
    movement_id: &str,
) -> AppResult<()> {
    let (task_id, snapshot, source_host_ref, identity) = runtime
        .native_agents
        .lock()
        .captured_result_snapshot_for_return(movement_id)?;
    let now = crate::storage::now_ts();
    let result = runtime.managed_objects.lock().acquire_new(
        crate::managed_objects::HostArtifactAcquisition {
            kind: crate::managed_objects::ManagedObjectAcquisitionKind::GeneratedArtifact,
            source_ref: format!("native-agent-result:{movement_id}"),
            bridge_id: Some(room_id.into()),
            path: snapshot.clone(),
            scope_root: snapshot.clone(),
            display_name: "native-agent-result".into(),
            media_type: "application/octet-stream".into(),
            expires_at: now + 60 * 60,
            app_owned_temporary: true,
        },
        now,
    )?;
    let mut package: Option<PathBuf> = None;
    let mut item_id: Option<String> = None;
    let sent: AppResult<()> = async {
        let prepared_package = crate::regular_file_set_transfer::prepare_package(
            &snapshot,
            &snapshot,
            &identity,
            &runtime.paths.temp_dir,
        )?;
        package = Some(prepared_package.clone());
        let source_host =
            crate::host_identity::HostRef::parse_peer(source_host_ref, &runtime.local_host_ref)?;
        let session = runtime
            .resolve_current_remote_host_session(room_id, &source_host)
            .await?;
        let master_key = {
            let config = runtime.config.read();
            crate::config::master_key(&config)?
        };
        let item = crate::storage::create_outgoing_file_item_with_metadata(
            &runtime.paths,
            &master_key,
            room_id,
            &prepared_package,
            Some("Codex result".into()),
            Some("application/octet-stream".into()),
        )?;
        item_id = Some(item.id.clone());
        let metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id,
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: room_id.into(),
            source_host_ref: runtime.local_host_ref.as_str().into(),
            destination_host_ref: source_host.as_str().into(),
            object: crate::bridge_plan_v2::ManagedObjectRevisionV2 {
                logical_object_id: result.object.logical_object_id.clone(),
                revision: result.object.revision,
            },
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        };
        crate::transfer::send_native_agent_workspace_to_current_remote_session(
            runtime.clone(),
            room_id,
            &item.id,
            &prepared_package,
            session,
            metadata,
        )
        .await
        .map_err(|failure| failure.error)
    }
    .await;

    if let Some(item_id) = item_id {
        let _ = crate::storage::delete_room_item(&runtime.paths, &item_id);
    }
    if let Some(package) = package.as_ref() {
        crate::regular_file_set_transfer::cleanup_package(package);
    }
    runtime.managed_objects.lock().discard_binding(&result);
    sent?;
    runtime
        .native_agents
        .lock()
        .complete_workspace_result_return(movement_id)
}

pub(crate) async fn retry_workspace_result_return(
    runtime: Arc<crate::host_runtime::HostRuntime>,
    room_id: &str,
    movement_id: &str,
) -> AppResult<()> {
    // This verifies the durable snapshot before any routing. The existing
    // Layer 4 resolver in send_workspace_result obtains a fresh current
    // transport binding, or fails closed if the old session was replaced.
    runtime
        .native_agents
        .lock()
        .validate_local_result_return_retry(room_id, movement_id)?;
    match send_workspace_result(runtime.clone(), room_id, movement_id).await {
        Ok(()) => Ok(()),
        Err(error) => {
            runtime
                .native_agents
                .lock()
                .mark_result_return_pending(movement_id);
            Err(error)
        }
    }
}

/// A remote Host's native capabilities as one fresh observation reports
/// them, judged by the protocols this Host would invoke each one with.
pub(crate) fn remote_native_capabilities(
    projection: &crate::peer_capabilities::PeerCapabilityProjection,
) -> Vec<NativeAgentCapabilityV1> {
    projection
        .native_agent_capability_ids()
        .into_iter()
        .map(|capability_id| {
            let (display_name, required): (&str, &[&str]) = if capability_id == CODEX_CAPABILITY_ID
            {
                ("Codex", &DIRECT_NATIVE_INVOKE_PROTOCOLS)
            } else {
                (capability_id, &GENERIC_NATIVE_INVOKE_PROTOCOLS)
            };
            let state = projection.native_agent_state(capability_id, required);
            NativeAgentCapabilityV1 {
                agent_id: capability_id.into(),
                display_name: display_name.into(),
                reason: (state != NativeAgentCapabilityStateV1::Available)
                    .then(|| projection.native_agent_reason(capability_id))
                    .flatten(),
                state,
                detail: None,
                backend: None,
            }
        })
        .collect()
}

pub(crate) fn validate_invoke(request: &NativeAgentInvokeV1) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || request.task_id.trim().is_empty()
        || request.task_id.len() > MAX_TASK_ID_BYTES
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
        || request.agent_capability != CODEX_CAPABILITY_ID
        || request.workspace.trim().is_empty()
        || request.workspace.len() > MAX_WORKSPACE_BYTES
        || request.task.trim().is_empty()
        || request.task.len() > MAX_TASK_BYTES
    {
        return invalid("Native Agent invocation is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_invoke_v2(request: &NativeAgentInvokeV2) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_INVOKE_V2_SCHEMA
        || !valid_task_id(&request.task_id)
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
        || crate::peer_capabilities::validate_semantic_capability_id(&request.agent_capability)
            .is_err()
        || request.input.validate().is_err()
    {
        return invalid("Native Agent invocation is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_status(status: &NativeAgentStatusV1) -> AppResult<()> {
    if status.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || status.task_id.trim().is_empty()
        || status.task_id.len() > MAX_TASK_ID_BYTES
        || status.executing_host_ref.trim().is_empty()
        || status.executing_host_ref.len() > 256
        || status.status.schema_version != "pastey-native-agent-task-v1"
        || status.status.task_id != status.task_id
        || crate::peer_capabilities::validate_semantic_capability_id(&status.status.agent_id)
            .is_err()
        || status
            .status
            .output
            .as_ref()
            .is_some_and(|output| output.validate().is_err())
        || status.status.workspace_name.trim().is_empty()
        || status.status.workspace_name.len() > 256
        || status.status.workspace_name.contains('/')
        || status.status.workspace_name.contains('\\')
        || !valid_lifecycle_code(status.status.code.as_deref())
        || status.status.native_decisions.len() > MAX_NATIVE_DECISION_REVIEWS
        || (status.status.state != NativeAgentTaskStateV1::Running
            && !status.status.native_decisions.is_empty())
        || !status
            .status
            .native_decisions
            .iter()
            .all(validate_native_decision_review)
    {
        return invalid("Native Agent status is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_cancel(cancel: &NativeAgentCancelV1) -> AppResult<()> {
    if cancel.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || cancel.task_id.trim().is_empty()
        || cancel.task_id.len() > MAX_TASK_ID_BYTES
        || cancel.target_host_ref.trim().is_empty()
        || cancel.target_host_ref.len() > 256
    {
        return invalid("Native Agent cancellation is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_reconcile(request: &NativeAgentReconcileV1) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || !valid_task_id(&request.task_id)
        || request
            .movement_id
            .as_ref()
            .is_some_and(|value| !valid_movement_id(value))
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
    {
        return invalid("Native Agent reconciliation query is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_retry_result_return(
    request: &NativeAgentRetryResultReturnV1,
) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || request.retry_id.is_empty()
        || request.retry_id.len() > 128
        || !request
            .retry_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !valid_movement_id(&request.movement_id)
        || !valid_task_id(&request.task_id)
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
    {
        return invalid("Native Agent result Return retry is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_reconciliation(fact: &NativeAgentReconciliationV1) -> AppResult<()> {
    if fact.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || !valid_task_id(&fact.task_id)
        || fact
            .movement_id
            .as_ref()
            .is_some_and(|value| !valid_movement_id(value))
        || fact.executing_host_ref.trim().is_empty()
        || fact.executing_host_ref.len() > 256
        || fact.result_digest.as_ref().is_some_and(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || !valid_lifecycle_code(fact.code.as_deref())
        || fact
            .task_output
            .as_ref()
            .is_some_and(|output| output.validate().is_err())
    {
        return invalid("Native Agent reconciliation fact is invalid.");
    }
    Ok(())
}

fn valid_lifecycle_code(code: Option<&str>) -> bool {
    code.is_none_or(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

pub(crate) fn validate_workspace_prepare(request: &NativeAgentWorkspacePrepareV1) -> AppResult<()> {
    if request.schema_version != NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA
        || !valid_movement_id(&request.movement_id)
        || !valid_task_id(&request.task_id)
        || request.source_host_ref.trim().is_empty()
        || request.source_host_ref.len() > 256
        || request.target_host_ref.trim().is_empty()
        || request.target_host_ref.len() > 256
        || request.agent_capability != CODEX_CAPABILITY_ID
        || request.task.trim().is_empty()
        || request.task.len() > MAX_TASK_BYTES
        || request.source_object.logical_object_id.trim().is_empty()
        || request.source_object.revision == 0
        || request.source_digest.trim().is_empty()
        || request.source_digest.len() > 128
        || request.source_bytes > crate::storage::MAX_FILE_SIZE_BYTES
    {
        return invalid("Native Agent workspace movement preparation is invalid.");
    }
    Ok(())
}

pub(crate) fn validate_workspace_transfer(
    metadata: &NativeAgentWorkspaceTransferV1,
) -> AppResult<()> {
    if metadata.schema_version != NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA
        || !valid_movement_id(&metadata.movement_id)
        || !valid_task_id(&metadata.task_id)
        || metadata.bridge_id.trim().is_empty()
        || metadata.bridge_id.len() > 256
        || metadata.source_host_ref.trim().is_empty()
        || metadata.source_host_ref.len() > 256
        || metadata.destination_host_ref.trim().is_empty()
        || metadata.destination_host_ref.len() > 256
        || metadata.source_host_ref == metadata.destination_host_ref
        || metadata.object.logical_object_id.trim().is_empty()
        || metadata.object.revision == 0
        || metadata.content_digest.trim().is_empty()
        || metadata.content_digest.len() > 128
        || metadata.logical_byte_count > crate::storage::MAX_FILE_SIZE_BYTES
    {
        return invalid("Native Agent workspace Transfer metadata is invalid.");
    }
    Ok(())
}

fn validate_movement_id(value: &str) -> AppResult<()> {
    if valid_movement_id(value) {
        Ok(())
    } else {
        invalid("Native Agent workspace movement identity is invalid.")
    }
}

fn validate_task_id(value: &str) -> AppResult<()> {
    if valid_task_id(value) {
        Ok(())
    } else {
        invalid("Native Agent task identity is invalid.")
    }
}

fn valid_movement_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_MOVEMENT_ID_BYTES
}

fn valid_task_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TASK_ID_BYTES
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;
    use std::{fs, sync::Arc};

    mod pair_harness {
        use super::*;
        include!("native_agent_pair_harness.rs");
    }

    struct NoopEventSink;
    impl crate::host_runtime::HostEventSink for NoopEventSink {
        fn emit(&self, _event: crate::host_runtime::HostEvent) -> AppResult<()> {
            Ok(())
        }
    }
    struct NoopTaskSpawner;
    impl crate::host_runtime::RuntimeTaskSpawner for NoopTaskSpawner {
        fn spawn(&self, _task: crate::host_runtime::RuntimeTask) {}
    }

    fn test_config() -> crate::config::StoredConfig {
        crate::config::StoredConfig {
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
            app_secret: crate::crypto::encode_key(&[9u8; 32]),
            device_id: "native-return-test".into(),
        }
    }

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        fixture_with_terminal(
            r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#,
        )
    }

    fn fixture_with_terminal(terminal: &str) -> (PathBuf, PathBuf, PathBuf) {
        fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; echo '{terminal}'"
        ))
    }

    fn fixture_with_turn_start_behavior(turn_start: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-native-agent-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let agent = root.join("codex-fixture");
        fs::write(
            &agent,
            format!(
                r#"#!/bin/sh
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) echo '{{"id":1,"result":{{}}}}' ;;
    *'"id":2'*) echo '{{"id":2,"result":{{"thread":{{"id":"native-thread"}}}}}}' ;;
    *'"id":3'*) {turn_start} ;;
  esac
done
"#
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&agent).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&agent, permissions).unwrap();
        }
        (root, workspace, agent)
    }

    fn blocked_turn_fixture() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-native-live-turn-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let agent = root.join("codex-live-turn-fixture");
        fs::write(
            &agent,
            r##"#!/bin/sh
# Waits end with the test process ($PPID), this app-server shell ($$) or
# its workspace, so no fixture process outlives its test, also on panic.
fixture_alive() { kill -0 "$PPID" 2>/dev/null && kill -0 "$$" 2>/dev/null && [ -d "$PWD" ]; }
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) echo '{"id":1,"result":{}}' ;;
    *'"id":2'*) echo '{"id":2,"result":{"thread":{"id":"native-thread"}}}' ;;
    *'"id":3'*)
      echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'
      touch "$PWD/turn-started"
      while [ ! -f "$PWD/release-turn" ]; do fixture_alive || exit 0; sleep 0.01; done
      echo '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'
      touch "$PWD/terminal-sent"
      ;;
    *'"method":"turn/interrupt"'*) touch "$PWD/turn-interrupt-requested" ;;
    *'"method":"shutdown"'*) touch "$PWD/session-shutdown-requested" ;;
  esac
done
"##,
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&agent).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&agent, permissions).unwrap();
        }
        (root, workspace, agent)
    }

    fn compatibility_fixture(root: &Path, app_server_help_exit: i32) -> PathBuf {
        let executable = root.join("codex-compatibility-fixture");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "codex fixture"; exit 0; fi
if [ "$1" = "app-server" ] && [ "$2" = "--help" ]; then exit {app_server_help_exit}; fi
exit 1
"#
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&executable, permissions).unwrap();
        }
        executable
    }

    /// A scripted app-server. `cases` are the shell `case` arms for the
    /// request lines Pastey writes; every invocation is logged outside the
    /// workspace so probes and launches can be counted.
    fn scripted_codex(cases: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-native-script-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let agent = root.join("codex-scripted-fixture");
        let script = r#"#!/bin/sh
# Waits end with the test process ($PPID), this app-server shell ($$) or
# its workspace, so no fixture process outlives its test, also on panic.
fixture_alive() { kill -0 "$PPID" 2>/dev/null && kill -0 "$$" 2>/dev/null && [ -d "$PWD" ]; }
printf '%s\n' "$*" >> '{ROOT}/invocations'
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
{CASES}
  esac
done
"#
        .replace("{ROOT}", root.to_str().unwrap())
        .replace("{CASES}", cases)
        .replace("{ACK}", SCRIPTED_ACK)
        .replace("{COMPLETED}", SCRIPTED_COMPLETED);
        fs::write(&agent, script).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&agent).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&agent, permissions).unwrap();
        }
        (root, workspace, agent)
    }

    const SCRIPTED_OPEN: &str = r#"    *'"id":1,'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"id":2,'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"native-thread"}}}' ;;
"#;
    const SCRIPTED_ACK: &str = r#"printf '%s\n' '{"id":3,"result":{"turn":{"id":"native-turn"}}}'"#;
    const SCRIPTED_COMPLETED: &str = r#"printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'"#;

    fn text_output(status: &NativeAgentTaskStatusV1) -> Option<(String, bool)> {
        let output = serde_json::to_value(status.output.as_ref()?).unwrap();
        assert_eq!(output["schemaVersion"], "pastey-native-text-output-v1");
        Some((
            output["text"].as_str().unwrap().to_owned(),
            output["truncated"].as_bool().unwrap(),
        ))
    }

    fn wait_for_code(service: &NativeAgentServiceV1, task_id: &str, code: Option<&str>) {
        for _ in 0..500 {
            if service.task_status(task_id).unwrap().code.as_deref() == code {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "task code never became {code:?}: {:?}",
            service.task_status(task_id).unwrap()
        )
    }

    #[cfg(unix)]
    fn process_alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(unix)]
    fn wait_for_process_exit(pid_file: &Path) {
        wait_for_file(pid_file);
        let pid = fs::read_to_string(pid_file).unwrap().trim().to_owned();
        for _ in 0..300 {
            if !process_alive(&pid) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid])
            .status();
        panic!("fixture wait {pid} outlived its owner");
    }

    /// A scripted app-server whose background (`bg`) and main-shell (`main`)
    /// waits never end on their own.
    #[cfg(unix)]
    fn endless_wait_fixture() -> (PathBuf, PathBuf, PathBuf) {
        scripted_codex(
            r#"    *bg*)
      ( while [ ! -f "$PWD/never" ]; do fixture_alive || exit 0; sleep 0.01; done ) &
      echo $! > "$PWD/waiter.pid"
      ;;
    *main*)
      echo $$ > "$PWD/waiter.pid"
      while [ ! -f "$PWD/never" ]; do fixture_alive || exit 0; sleep 0.01; done
      ;;
"#,
        )
    }

    #[cfg(unix)]
    #[test]
    fn fixture_waits_never_outlive_their_app_server_workspace_or_test() {
        use std::io::Write as _;
        // The app-server shell is killed, as Pastey's shutdown does.
        let (root, workspace, agent) = endless_wait_fixture();
        let mut child = std::process::Command::new(&agent)
            .current_dir(&workspace)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(b"bg\n").unwrap();
        wait_for_file(&workspace.join("waiter.pid"));
        child.kill().unwrap();
        child.wait().unwrap();
        wait_for_process_exit(&workspace.join("waiter.pid"));
        let _ = fs::remove_dir_all(root);

        // The workspace is removed while a main-shell wait runs.
        let (root, workspace, agent) = endless_wait_fixture();
        let mut child = std::process::Command::new(&agent)
            .current_dir(&workspace)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(b"main\n").unwrap();
        wait_for_file(&workspace.join("waiter.pid"));
        let pid = fs::read_to_string(workspace.join("waiter.pid")).unwrap();
        fs::remove_dir_all(&root).unwrap();
        for _ in 0..300 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            child.try_wait().unwrap().is_some(),
            "main wait {pid} survived"
        );

        // The process that started it goes away (a test that panicked
        // without cleanup) while its workspace remains.
        for wait in ["bg", "main"] {
            let (root, workspace, agent) = endless_wait_fixture();
            std::process::Command::new("/bin/sh")
                .current_dir(&workspace)
                .args([
                    "-c",
                    &format!("(echo {wait}; sleep 5) | '{}' & sleep 0.5", agent.display()),
                ])
                .status()
                .unwrap();
            wait_for_process_exit(&workspace.join("waiter.pid"));
            let _ = fs::remove_dir_all(root);
        }
    }

    fn wait_for_file(path: &Path) {
        for _ in 0..500 {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{} was never written", path.display())
    }

    fn invocation_count(root: &Path, invocation: &str) -> usize {
        fs::read_to_string(root.join("invocations"))
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == invocation)
            .count()
    }

    fn final_answer_fixture() -> (PathBuf, PathBuf, PathBuf) {
        scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"native-thread","turnId":"native-turn","itemId":"m1","delta":"DELTA-ONLY"}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"reasoning","id":"r1","summary":["REASONING-SECRET"],"content":["REASONING-SECRET"]}}}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"commandExecution","id":"c1","command":"env","aggregatedOutput":"TOOL-LOG-SECRET"}}}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"m0","text":"COMMENTARY-ONLY","phase":"commentary"}}}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"other-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"x1","text":"OTHER-THREAD","phase":"final_answer"}}}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"other-turn","item":{{"type":"agentMessage","id":"x2","text":"OTHER-TURN","phase":"final_answer"}}}}}}'
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"m1","text":"Answer <b>bold</b> & \"quoted\" ✓\nsecond line","phase":"final_answer"}}}}}}'
      {{COMPLETED}}
      ;;
"#
        ))
    }

    #[test]
    fn codex_returns_only_the_exact_turns_final_user_facing_answer() {
        let (root, workspace, agent) = final_answer_fixture();
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "answer me")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(terminal.code, None);
        assert_eq!(
            text_output(&terminal),
            Some((
                "Answer <b>bold</b> & \"quoted\" ✓\nsecond line".into(),
                false
            ))
        );
        assert_eq!(
            terminal.result.as_deref(),
            Some("Codex completed its native task.")
        );
        // Nothing but the exact final answer crosses: no other thread or
        // turn, no delta, reasoning, tool output, commentary or session id.
        let projected = serde_json::to_string(&terminal).unwrap();
        for leaked in [
            "OTHER-THREAD",
            "OTHER-TURN",
            "DELTA-ONLY",
            "REASONING-SECRET",
            "TOOL-LOG-SECRET",
            "COMMENTARY-ONLY",
            "native-thread",
            "native-turn",
        ] {
            assert!(!projected.contains(leaked), "{leaked} leaked: {projected}");
        }
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_malformed_and_oversized_responses_never_fabricate_output() {
        // No agent message at all.
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "silent")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(terminal.output, None);
        assert_eq!(
            terminal.result.as_deref(),
            Some("Codex completed its native task without a final response.")
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);

        // Unreadable lines are skipped without losing observation, and are
        // reported instead of an answer.
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' 'this is not json'
      head -c 1100000 /dev/zero | tr '\000' 'x'; printf '\n'
      {{COMPLETED}}
      ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "unreadable")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(terminal.output, None);
        assert_eq!(
            terminal.result.as_deref(),
            Some("Codex completed its native task, but Pastey could not read its final response.")
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);

        // An answer larger than the output bound is cut, marked, and valid.
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"m","phase":"final_answer","text":"'
      head -c 40000 /dev/zero | tr '\000' 'a'
      printf '%s\n' '"}}}}}}'
      {{COMPLETED}}
      ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "long answer")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        let output = terminal.output.as_ref().unwrap();
        output.validate().unwrap();
        let (text, truncated) = text_output(&terminal).unwrap();
        assert!(truncated);
        assert!(text.len() > 15_000 && text.bytes().all(|byte| byte == b'a'));
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_wins_a_racing_native_answer_and_completion() {
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"m","text":"LATE-ANSWER","phase":"final_answer"}}}}}}'
      touch "$PWD/answer-sent"
      ( while [ ! -f "$PWD/release-turn" ]; do fixture_alive || exit 0; sleep 0.01; done; {{COMPLETED}} ) &
      ;;
    *'turn/interrupt'*) touch "$PWD/interrupt-requested" ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "race")
            .unwrap();
        wait_for_file(&workspace.join("answer-sent"));
        let canonical = workspace.canonicalize().unwrap();
        let controller = service.codex.wait_for_session_controller(&canonical);
        for _ in 0..300 {
            if controller.active_turn.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            service.cancel_task(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        wait_for_file(&workspace.join("interrupt-requested"));
        // Codex finishes anyway: its answer and completion arrive late.
        fs::write(workspace.join("release-turn"), b"").unwrap();
        wait_for_workspace_release(&service, &workspace);
        let after = service.task_status(&started.task_id).unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.output, None);
        assert_eq!(after.result, None);
        drop(controller);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_approval_wait_is_surfaced_unanswered_until_cancel_interrupts_it() {
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' '{{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{{"threadId":"native-thread","turnId":"native-turn","itemId":"cmd-1","command":"touch ../outside"}}}}'
      printf '%s\n' '{{"id":"tool-1","method":"item/tool/call","params":{{"threadId":"native-thread","turnId":"native-turn","callId":"call-1","tool":"x","arguments":{{}}}}}}'
      ;;
    *'"id":"approval-1"'*) printf '%s\n' "$line" > "$PWD/approval-answered" ;;
    *'"id":"tool-1"'*) printf '%s\n' "$line" > "$PWD/tool-call-refused" ;;
    *'turn/interrupt'*)
      printf '%s\n' '{{"method":"serverRequest/resolved","params":{{"threadId":"native-thread","requestId":"approval-1"}}}}'
      printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"native-thread","turn":{{"id":"native-turn","items":[],"status":"interrupted","error":null}}}}}}'
      ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "needs approval")
            .unwrap();
        wait_for_code(
            &service,
            &started.task_id,
            Some(AWAITING_NATIVE_DECISION_CODE),
        );
        // A client capability Pastey never offered is refused by protocol.
        wait_for_file(&workspace.join("tool-call-refused"));
        let refusal = fs::read_to_string(workspace.join("tool-call-refused")).unwrap();
        assert!(refusal.contains("-32601"), "{refusal}");
        // Still running, still occupying its workspace, and never answered.
        thread::sleep(Duration::from_millis(200));
        let waiting = service.task_status(&started.task_id).unwrap();
        assert_eq!(waiting.state, NativeAgentTaskStateV1::Running);
        assert_eq!(waiting.output, None);
        assert!(service
            .workspace_occupant(&workspace.canonicalize().unwrap())
            .is_some());
        assert!(!workspace.join("approval-answered").exists());

        let cancelled = service.cancel_task(&started.task_id).unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        wait_for_workspace_release(&service, &workspace);
        let after = service.task_status(&started.task_id).unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.code.as_deref(), Some("native_agent_cancel_requested"));
        assert!(!workspace.join("approval-answered").exists());
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn natively_resolved_approval_returns_to_running_and_completes_truthfully() {
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' '{{"id":7,"method":"item/fileChange/requestApproval","params":{{"threadId":"native-thread","turnId":"native-turn","itemId":"patch-1"}}}}'
      (
        while [ ! -f "$PWD/resolve-approval" ]; do fixture_alive || exit 0; sleep 0.01; done
        printf '%s\n' '{{"method":"serverRequest/resolved","params":{{"threadId":"native-thread","requestId":7}}}}'
        while [ ! -f "$PWD/finish-turn" ]; do fixture_alive || exit 0; sleep 0.01; done
        printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"native-thread","turnId":"native-turn","item":{{"type":"agentMessage","id":"m","text":"Done after approval."}}}}}}'
        {{COMPLETED}}
      ) &
      ;;
    *'"id":7'*) printf '%s\n' "$line" > "$PWD/approval-answered" ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "patch")
            .unwrap();
        wait_for_code(
            &service,
            &started.task_id,
            Some(AWAITING_NATIVE_DECISION_CODE),
        );
        fs::write(workspace.join("resolve-approval"), b"").unwrap();
        wait_for_code(&service, &started.task_id, None);
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Running
        );
        fs::write(workspace.join("finish-turn"), b"").unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(terminal.code, None);
        assert_eq!(
            text_output(&terminal),
            Some(("Done after approval.".into(), false))
        );
        assert!(!workspace.join("approval-answered").exists());
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn slow_session_opening_never_holds_start_status_or_cancel() {
        let (root, workspace, agent) = scripted_codex(
            r#"    *'"id":1,'*)
      while [ ! -f "$PWD/release-initialize" ]; do fixture_alive || exit 0; sleep 0.01; done
      printf '%s\n' '{"id":1,"result":{}}'
      ;;
    *'"id":2,'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"native-thread"}}}' ;;
    *'"id":3,'*) touch "$PWD/turn-start-received"; {ACK} ;;
"#,
        );
        let service = Arc::new(parking_lot::Mutex::new(NativeAgentServiceV1::default()));
        // `start` must return while initialize is still unanswered; if it
        // waited for the app-server it would never return here.
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let service = service.clone();
            let agent = agent.clone();
            let workspace = workspace.clone();
            thread::spawn(move || {
                let started = service.lock().start_codex_task_with_executable(
                    &agent,
                    &workspace,
                    "slow open",
                );
                let _ = sender.send(started);
            });
        }
        let started = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("start waited for the native app-server")
            .unwrap();
        assert_eq!(started.state, NativeAgentTaskStateV1::Running);
        assert!(!started.session_reused);
        // The service lock stays free for status and cancellation.
        let status_at = std::time::Instant::now();
        let status = service.lock().task_status(&started.task_id).unwrap();
        assert_eq!(status.state, NativeAgentTaskStateV1::Running);
        let cancelled = service.lock().cancel_task(&started.task_id).unwrap();
        assert!(status_at.elapsed() < Duration::from_secs(2));
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            cancelled.code.as_deref(),
            Some("native_agent_cancel_requested")
        );
        // Occupancy holds until the opening session is closed, and the turn
        // is never sent.
        assert!(service
            .lock()
            .workspace_occupant(&workspace.canonicalize().unwrap())
            .is_some());
        fs::write(workspace.join("release-initialize"), b"").unwrap();
        for _ in 0..500 {
            if service
                .lock()
                .workspace_occupant(&workspace.canonicalize().unwrap())
                .is_none()
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let service = service.lock();
        assert!(service
            .workspace_occupant(&workspace.canonicalize().unwrap())
            .is_none());
        let after = service.task_status(&started.task_id).unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.code.as_deref(), Some("native_agent_cancelled"));
        thread::sleep(Duration::from_millis(100));
        assert!(!workspace.join("turn-start-received").exists());
        assert_eq!(service.codex.session_count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn availability_is_cached_per_executable_and_rechecked_fresh_at_execution() {
        let (root, workspace, agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*) {{ACK}}; {{COMPLETED}} ;;
"#
        ));
        let mut service = NativeAgentServiceV1::default();
        service.codex.use_executable(&agent);
        for _ in 0..5 {
            assert_eq!(
                service.codex.availability(),
                NativeAgentCapabilityStateV1::Available
            );
            assert_eq!(
                service.capabilities()[0].state,
                NativeAgentCapabilityStateV1::Available
            );
        }
        assert_eq!(invocation_count(&root, "--version"), 1);
        assert_eq!(invocation_count(&root, "app-server --help"), 1);
        // Admission reuses the observation; opening a new session observes
        // it afresh exactly once.
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "count probes")
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &started.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(invocation_count(&root, "--version"), 2);
        assert_eq!(invocation_count(&root, "app-server --stdio"), 1);
        // A reused session needs no new probe or launch.
        let second = service
            .start_codex_task_with_executable(&agent, &workspace, "again")
            .unwrap();
        assert!(second.session_reused);
        wait_for_terminal(&service, &second.task_id);
        assert_eq!(invocation_count(&root, "--version"), 2);
        assert_eq!(invocation_count(&root, "app-server --stdio"), 1);
        // Another executable is a different fact.
        service.codex.use_executable(&root.join("codex-absent"));
        assert_eq!(
            service.codex.availability(),
            NativeAgentCapabilityStateV1::Unavailable
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_session_that_cannot_open_is_a_definite_non_start() {
        let (root, workspace, agent) = scripted_codex(
            r#"    *'"id":1,'*) printf '%s\n' '{"id":1,"error":{"code":-1,"message":"not signed in"}}' ;;
    *'"id":3,'*) touch "$PWD/turn-start-received" ;;
"#,
        );
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "never runs")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Failed);
        assert_eq!(terminal.code.as_deref(), Some("native_agent_start_failed"));
        assert_eq!(
            terminal.result.as_deref(),
            Some("Codex could not start its native app-server.")
        );
        assert_eq!(terminal.output, None);
        wait_for_workspace_release(&service, &workspace);
        assert!(!workspace.join("turn-start-received").exists());
        assert_eq!(service.codex.session_count(), 0);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_requester_shows_reviews_and_refuses_stale_decisions_locally() {
        let bridge = "room-requester-review";
        let executor_host = "executor-host";
        let task_id = "native-agent:remote-review";
        let mut requester = NativeAgentServiceV1::default();
        requester
            .queue_bridge_remote_task(bridge, task_id, executor_host, r"C:\REPZ", "work")
            .unwrap();
        let content = NativeDecisionContentV1 {
            command: Some("cargo test".into()),
            cwd: Some(r"C:\REPZ".into()),
            ..NativeDecisionContentV1::default()
        };
        let review = NativeDecisionReviewV1 {
            review_ref: "native-decision:abc".into(),
            kind: NativeDecisionKindV1::CommandExecution,
            state: NativeDecisionStateV1::Pending,
            submitted: None,
            can_accept: true,
            can_decline: true,
            unavailable_reason: None,
            content_digest: native_decision_content_digest(
                NativeDecisionKindV1::CommandExecution,
                Some(&content),
            ),
            content: Some(content),
            requested_at: 1,
            expires_at: i64::MAX,
        };
        let running = |reviews: Vec<NativeDecisionReviewV1>, state| NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: task_id.into(),
            executing_host_ref: executor_host.into(),
            status: NativeAgentTaskStatusV1 {
                schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: task_id.into(),
                agent_id: CODEX_CAPABILITY_ID.into(),
                workspace_name: "REPZ".into(),
                session_reused: false,
                state,
                result: None,
                code: Some(AWAITING_NATIVE_DECISION_CODE.into()),
                output: None,
                conversation: None,
                native_decisions: reviews,
            },
        };
        let recorded = requester
            .record_bridge_remote_status(
                bridge,
                running(vec![review.clone()], NativeAgentTaskStateV1::Running),
            )
            .unwrap();
        assert_eq!(recorded.native_decisions, [review.clone()]);
        let digest = review.content_digest.clone();
        assert_eq!(
            requester
                .remote_decision_target(
                    bridge,
                    task_id,
                    &review.review_ref,
                    &digest,
                    NativeDecisionV1::Accept
                )
                .unwrap(),
            executor_host
        );
        // Another Bridge, another review, other content.
        assert!(requester
            .remote_decision_target(
                "room-x",
                task_id,
                &review.review_ref,
                &digest,
                NativeDecisionV1::Accept
            )
            .is_err());
        assert!(requester
            .remote_decision_target(
                bridge,
                task_id,
                "native-decision:zzz",
                &digest,
                NativeDecisionV1::Accept
            )
            .is_err());
        assert!(requester
            .remote_decision_target(
                bridge,
                task_id,
                &review.review_ref,
                &"0".repeat(64),
                NativeDecisionV1::Accept
            )
            .is_err());
        // Once the executor reports it submitted, the click is stale.
        let mut submitted = review.clone();
        submitted.state = NativeDecisionStateV1::Submitted;
        submitted.submitted = Some(NativeDecisionV1::Decline);
        submitted.can_accept = false;
        submitted.can_decline = false;
        requester
            .record_bridge_remote_status(
                bridge,
                running(vec![submitted], NativeAgentTaskStateV1::Running),
            )
            .unwrap();
        assert!(requester
            .remote_decision_target(
                bridge,
                task_id,
                &review.review_ref,
                &digest,
                NativeDecisionV1::Accept
            )
            .is_err());
        // A terminal status never carries reviews.
        assert!(requester
            .record_bridge_remote_status(
                bridge,
                running(vec![review.clone()], NativeAgentTaskStateV1::Completed)
            )
            .is_err());
        // The requester's own Stop ends it locally at once.
        requester
            .record_bridge_remote_status(
                bridge,
                running(vec![review.clone()], NativeAgentTaskStateV1::Running),
            )
            .unwrap();
        let cancelled = requester
            .cancel_remote_task(task_id, executor_host)
            .unwrap();
        assert!(cancelled.native_decisions.is_empty());
        assert!(requester
            .remote_decision_target(
                bridge,
                task_id,
                &review.review_ref,
                &digest,
                NativeDecisionV1::Decline
            )
            .is_err());
    }

    #[test]
    fn remote_status_and_reconciliation_carry_the_exact_response_to_the_requester() {
        let (root, workspace, agent) = final_answer_fixture();
        let bridge = "room-remote-output";
        let executor_host = "executor-host";
        let mut executor = NativeAgentServiceV1::default();
        let mut requester = NativeAgentServiceV1::default();
        let task_id = "native-agent:remote-output";
        requester
            .queue_bridge_remote_task(
                bridge,
                task_id,
                executor_host,
                r"C:\Users\someone\project",
                "answer me",
            )
            .unwrap();
        start_accepted_bridge_codex_task(
            &mut executor,
            bridge,
            &agent,
            task_id,
            &workspace,
            "answer me",
        )
        .unwrap();
        let wire = |status: NativeAgentTaskStatusV1| {
            let message = NativeAgentStatusV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: task_id.into(),
                executing_host_ref: executor_host.into(),
                status,
            };
            let wire: NativeAgentStatusV1 =
                serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
            validate_status(&wire).unwrap();
            wire
        };
        // A native approval wait reaches the requester while still running.
        let mut waiting = executor.task_status(task_id).unwrap();
        waiting.state = NativeAgentTaskStateV1::Running;
        waiting.code = Some(AWAITING_NATIVE_DECISION_CODE.into());
        let recorded = requester
            .record_bridge_remote_status(bridge, wire(waiting))
            .unwrap();
        assert_eq!(recorded.state, NativeAgentTaskStateV1::Running);
        assert_eq!(
            recorded.code.as_deref(),
            Some(AWAITING_NATIVE_DECISION_CODE)
        );

        let terminal = wait_for_terminal(&executor, task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        let recorded = requester
            .record_bridge_remote_status(bridge, wire(terminal.clone()))
            .unwrap();
        assert_eq!(recorded.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(recorded.output, terminal.output);
        assert_eq!(
            text_output(&recorded).unwrap().0,
            "Answer <b>bold</b> & \"quoted\" ✓\nsecond line"
        );
        // A stale running fact never reopens it or drops its response.
        let mut stale = terminal.clone();
        stale.state = NativeAgentTaskStateV1::Running;
        stale.output = None;
        let kept = requester
            .record_bridge_remote_status(bridge, wire(stale))
            .unwrap();
        assert_eq!(kept.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(kept.output, terminal.output);
        // Reconciliation carries the same opaque response.
        let fact = executor
            .reconciliation_fact(bridge, task_id, None, executor_host, "requester-host")
            .unwrap();
        assert_eq!(fact.task_output, terminal.output);

        // A requester that cancelled never adopts a late completion's output.
        let cancelled_task = "native-agent:remote-cancelled";
        requester
            .queue_bridge_remote_task(
                bridge,
                cancelled_task,
                executor_host,
                r"C:\Users\someone\project",
                "answer me",
            )
            .unwrap();
        requester
            .cancel_remote_task(cancelled_task, executor_host)
            .unwrap();
        let mut late = terminal.clone();
        late.task_id = cancelled_task.into();
        let message = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: cancelled_task.into(),
            executing_host_ref: executor_host.into(),
            status: late,
        };
        let after = requester
            .record_bridge_remote_status(bridge, message)
            .unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.output, None);
        let mut late_fact = fact.clone();
        late_fact.task_id = cancelled_task.into();
        requester
            .record_bridge_remote_reconciliation(bridge, late_fact)
            .unwrap();
        let after = requester.task_status(cancelled_task).unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.output, None);
        executor.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_capability_state_is_the_selected_hosts_advertised_fact() {
        let projection = |capabilities| crate::peer_capabilities::PeerCapabilityProjection {
            schema_version: crate::peer_capabilities::PEER_CAPABILITY_SCHEMA.into(),
            peer_session_id: "peer".into(),
            observed_at: 1,
            capabilities,
        };
        let state_of = |facts| {
            remote_native_capabilities(&projection(facts))
                .into_iter()
                .map(|capability| {
                    (
                        capability.agent_id,
                        capability.display_name,
                        capability.state,
                    )
                })
                .collect::<Vec<_>>()
        };
        let fact = |id: &str, state: NativeAgentCapabilityStateV1, protocols: &[&str]| {
            crate::peer_capabilities::native_agent_capability_fact(id, state, None, protocols)
        };
        assert_eq!(
            state_of(vec![fact(
                CODEX_CAPABILITY_ID,
                NativeAgentCapabilityStateV1::Available,
                &WORKSPACE_MOVEMENT_PROTOCOLS
            )]),
            vec![(
                CODEX_CAPABILITY_ID.to_owned(),
                "Codex".to_owned(),
                NativeAgentCapabilityStateV1::Available
            )]
        );
        // Advertised without the protocols this Host invokes it with.
        assert_eq!(
            state_of(vec![fact(
                CODEX_CAPABILITY_ID,
                NativeAgentCapabilityStateV1::Available,
                &[NATIVE_AGENT_PROTOCOL_SCHEMA]
            )])[0]
                .2,
            NativeAgentCapabilityStateV1::Incompatible
        );
        for state in [
            NativeAgentCapabilityStateV1::Incompatible,
            NativeAgentCapabilityStateV1::Unavailable,
        ] {
            assert_eq!(
                state_of(vec![fact(
                    CODEX_CAPABILITY_ID,
                    state.clone(),
                    &WORKSPACE_MOVEMENT_PROTOCOLS
                )])[0]
                    .2,
                state
            );
        }
        // Other native capabilities keep their own ids and generic protocols;
        // non-native facts are not native capabilities at all.
        let mut facts = vec![fact(
            "fake.longjob.v1",
            NativeAgentCapabilityStateV1::Available,
            &GENERIC_NATIVE_INVOKE_PROTOCOLS,
        )];
        facts.push(crate::peer_capabilities::HostCapabilityFact {
            capability_id: "pastey.managed.runtime".into(),
            available: true,
            accepted_input_media_types: Vec::new(),
            effect: "readiness_observation".into(),
            supported_protocols: Vec::new(),
            unavailable_reason: None,
        });
        assert_eq!(
            state_of(facts),
            vec![(
                "fake.longjob.v1".to_owned(),
                "fake.longjob.v1".to_owned(),
                NativeAgentCapabilityStateV1::Available
            )]
        );
        // No observed native capability means nothing to offer.
        assert!(state_of(Vec::new()).is_empty());
    }

    /// An app-server that keeps its threads on disk like Codex's rollouts:
    /// `thread/start` creates one, `thread/resume` reopens only a stored one
    /// (or misbehaves as `resume-mode` says), and each turn answers with the
    /// thread it ran on. Every native step is logged outside the workspace.
    fn conversation_codex() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-native-conv-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let agent = root.join("codex-conversation-fixture");
        let script = r#"#!/bin/sh
# Waits end with the test process ($PPID) or this app-server shell ($$);
# deleting the fixture root ends them too.
fixture_alive() { kill -0 "$PPID" 2>/dev/null && kill -0 "$$" 2>/dev/null; }
printf '%s\n' "$*" >> '{ROOT}/invocations'
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
STORE='{ROOT}/threads'
mkdir -p "$STORE"
field() { printf '%s' "$line" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"; }
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"method":"thread/start"'*)
      n=$(cat '{ROOT}/counter' 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > '{ROOT}/counter'
      tid="thread-$n"; touch "$STORE/$tid"
      printf '%s\n' "start $tid" >> '{ROOT}/log'
      printf '{"id":2,"result":{"thread":{"id":"%s"}}}\n' "$tid" ;;
    *'"method":"thread/resume"'*)
      tid=$(field threadId)
      printf '%s\n' "resume $tid" >> '{ROOT}/log'
      while [ -f '{ROOT}/hold-resume' ]; do fixture_alive || exit 0; sleep 0.01; done
      mode=$(cat '{ROOT}/resume-mode' 2>/dev/null)
      if [ "$mode" = unsupported ]; then printf '%s\n' '{"id":6,"error":{"code":-32601,"message":"method not found"}}'
      elif [ "$mode" = other ]; then printf '%s\n' '{"id":6,"result":{"thread":{"id":"thread-other"}}}'
      elif [ -f "$STORE/$tid" ]; then printf '{"id":6,"result":{"thread":{"id":"%s"}}}\n' "$tid"
      else printf '%s\n' '{"id":6,"error":{"code":-32600,"message":"thread not found"}}'; fi ;;
    *'"method":"turn/start"'*)
      tid=$(field threadId); task=$(field text)
      printf '%s\n' "turn $tid $task" >> '{ROOT}/log'
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn-x"}}}'
      while [ -f '{ROOT}/hold-turn' ]; do fixture_alive || exit 0; sleep 0.01; done
      printf '{"method":"item/completed","params":{"threadId":"%s","turnId":"turn-x","item":{"type":"agentMessage","id":"m","phase":"final_answer","text":"%s on %s"}}}\n' "$tid" "$task" "$tid"
      printf '{"method":"turn/completed","params":{"threadId":"%s","turn":{"id":"turn-x","items":[],"status":"completed","error":null}}}\n' "$tid" ;;
  esac
done
"#
        .replace("{ROOT}", root.to_str().unwrap());
        fs::write(&agent, script).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&agent).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&agent, permissions).unwrap();
        }
        (root, workspace, agent)
    }

    fn native_log(root: &Path) -> Vec<String> {
        fs::read_to_string(root.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// A Codex task owned by one authenticated peer Host over one Bridge,
    /// as its accepted Review starts it.
    fn start_peer_codex_task(
        service: &mut NativeAgentServiceV1,
        agent: &Path,
        bridge_id: &str,
        peer_host_ref: &str,
        task_id: &str,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        service.codex.use_executable(agent);
        let input = capability::codex::codex_input(workspace, task, resume)?;
        service.with_bridge_task_owner(bridge_id, task_id, |service| {
            service.start_invocation_in_movement(
                CODEX_CAPABILITY_ID,
                task_id,
                &input,
                None,
                Some(NativeConversationOwnerV1::Peer {
                    bridge_id: bridge_id.into(),
                    peer_host_ref: peer_host_ref.into(),
                }),
            )
        })
    }

    fn local_conversation(workspace: &Path) -> NativeConversationBindingV1 {
        NativeConversationBindingV1::new(
            NativeConversationOwnerV1::Local,
            CODEX_CAPABILITY_ID,
            &workspace.canonicalize().unwrap().to_string_lossy(),
        )
        .unwrap()
    }

    fn run_local(
        service: &mut NativeAgentServiceV1,
        agent: &Path,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> NativeAgentTaskStatusV1 {
        let started = service
            .start_codex_task_with_executable_id_and_resume(
                agent,
                &format!("native-agent:{}", Uuid::new_v4()),
                &workspace.canonicalize().unwrap(),
                task,
                resume,
            )
            .unwrap();
        let terminal = wait_for_terminal(service, &started.task_id);
        wait_for_workspace_release(service, workspace);
        terminal
    }

    #[test]
    fn a_pastey_conversation_is_saved_once_created_and_continued_after_restart() {
        let (root, workspace, agent) = conversation_codex();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let first = run_local(&mut service, &agent, &workspace, "first", true);
        assert_eq!(first.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(
            first.conversation,
            Some(NativeConversationContinuityV1::New)
        );
        assert!(!first.session_reused);
        assert_eq!(text_output(&first).unwrap().0, "first on thread-1");
        // Saved after confirmed creation, Host-private, never in the status.
        let saved = local_conversation(&workspace)
            .saved_continuation(&paths)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            json!({"threadId": "thread-1"})
        );
        // The status says only "new"; the thread id stays Host-private.
        let projected = serde_json::to_value(&first).unwrap();
        assert_eq!(projected["conversation"], "new");
        assert!(!serde_json::to_string(&projected)
            .unwrap()
            .contains("threadId"));
        // The live session continues without reopening.
        let live = run_local(&mut service, &agent, &workspace, "second", true);
        assert_eq!(
            live.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert!(live.session_reused);
        assert_eq!(text_output(&live).unwrap().0, "second on thread-1");
        service.shutdown();
        drop(service);

        // Pastey restarts: the same conversation is reopened natively.
        let mut restarted = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let started = restarted
            .start_codex_task_with_executable_id_and_resume(
                &agent,
                "native-agent:after-restart",
                &workspace.canonicalize().unwrap(),
                "third",
                true,
            )
            .unwrap();
        // Not claimed before Codex confirms it.
        assert_eq!(started.conversation, None);
        assert!(!started.session_reused);
        let continued = wait_for_terminal(&restarted, &started.task_id);
        assert_eq!(continued.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(
            continued.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert!(continued.session_reused);
        assert_eq!(text_output(&continued).unwrap().0, "third on thread-1");
        assert_eq!(
            native_log(&root),
            [
                "start thread-1",
                "turn thread-1 first",
                "turn thread-1 second",
                "resume thread-1",
                "turn thread-1 third",
            ]
        );
        restarted.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn another_owner_or_workspace_never_inherits_a_conversation() {
        let (root, workspace, agent) = conversation_codex();
        let other_workspace = root.join("other");
        fs::create_dir(&other_workspace).unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let local = run_local(&mut service, &agent, &workspace, "local", true);
        assert_eq!(text_output(&local).unwrap().0, "local on thread-1");

        // The same workspace path, but a peer Host: its own conversation.
        let peer_task = |service: &mut NativeAgentServiceV1,
                         bridge: &str,
                         peer: &str,
                         task_id: &str,
                         workspace: &Path,
                         task: &str| {
            let started = start_peer_codex_task(
                service, &agent, bridge, peer, task_id, workspace, task, true,
            )
            .unwrap();
            let terminal = wait_for_terminal(service, &started.task_id);
            wait_for_workspace_release(service, workspace);
            terminal
        };
        let peer_a = peer_task(
            &mut service,
            "room-a",
            "host:mac",
            "task-a1",
            &workspace,
            "peera",
        );
        assert_eq!(
            peer_a.conversation,
            Some(NativeConversationContinuityV1::New)
        );
        assert_eq!(text_output(&peer_a).unwrap().0, "peera on thread-2");
        // Another Host on the same Bridge, and the same Host on another
        // Bridge, are other principals.
        let peer_b = peer_task(
            &mut service,
            "room-a",
            "host:other",
            "task-b1",
            &workspace,
            "peerb",
        );
        assert_eq!(text_output(&peer_b).unwrap().0, "peerb on thread-3");
        let other_bridge = peer_task(
            &mut service,
            "room-b",
            "host:mac",
            "task-c1",
            &workspace,
            "bridgeb",
        );
        assert_eq!(text_output(&other_bridge).unwrap().0, "bridgeb on thread-4");
        // Another workspace of the same owner is another conversation.
        let elsewhere = run_local(&mut service, &agent, &other_workspace, "elsewhere", true);
        assert_eq!(
            elsewhere.conversation,
            Some(NativeConversationContinuityV1::New)
        );
        service.shutdown();
        drop(service);

        // After restart each owner reopens only its own thread.
        let mut restarted = NativeAgentServiceV1::with_paths(paths).unwrap();
        let again_a = peer_task(
            &mut restarted,
            "room-a",
            "host:mac",
            "task-a2",
            &workspace,
            "again",
        );
        assert_eq!(
            again_a.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert_eq!(text_output(&again_a).unwrap().0, "again on thread-2");
        let again_local = run_local(&mut restarted, &agent, &workspace, "mine", true);
        assert_eq!(text_output(&again_local).unwrap().0, "mine on thread-1");
        // A peer that never had a conversation here gets a new one.
        let stranger = peer_task(
            &mut restarted,
            "room-a",
            "host:stranger",
            "task-s1",
            &workspace,
            "hi",
        );
        assert_eq!(
            stranger.conversation,
            Some(NativeConversationContinuityV1::New)
        );
        assert_eq!(text_output(&stranger).unwrap().0, "hi on thread-6");
        restarted.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unavailable_conversation_is_never_replaced_by_a_new_one() {
        for mode in ["missing", "unsupported", "other"] {
            let (root, workspace, agent) = conversation_codex();
            let paths = durable_paths(&root);
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            run_local(&mut service, &agent, &workspace, "first", true);
            service.shutdown();
            drop(service);
            match mode {
                // Codex no longer has the thread (deleted or unreadable).
                "missing" => fs::remove_file(root.join("threads").join("thread-1")).unwrap(),
                // An app-server without thread/resume, or one that answers
                // with another thread.
                _ => fs::write(root.join("resume-mode"), mode).unwrap(),
            }
            let mut restarted = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            let unavailable = run_local(&mut restarted, &agent, &workspace, "follow", true);
            assert_eq!(unavailable.state, NativeAgentTaskStateV1::Failed, "{mode}");
            assert_eq!(
                unavailable.code.as_deref(),
                Some(CONVERSATION_UNAVAILABLE_CODE),
                "{mode}"
            );
            assert_eq!(unavailable.conversation, None);
            assert!(unavailable
                .result
                .unwrap()
                .contains("Start a new conversation"));
            // No new thread, and no turn ran.
            assert_eq!(
                native_log(&root),
                ["start thread-1", "turn thread-1 first", "resume thread-1"],
                "{mode}"
            );
            wait_for_workspace_release(&restarted, &workspace);
            // The record is kept: continuing again is still truthful.
            assert!(local_conversation(&workspace)
                .saved_continuation(&paths)
                .unwrap()
                .is_some());
            // Only an explicit new conversation starts one, and it becomes
            // the one later tasks continue.
            let _ = fs::remove_file(root.join("resume-mode"));
            let fresh = run_local(&mut restarted, &agent, &workspace, "fresh", false);
            assert_eq!(fresh.state, NativeAgentTaskStateV1::Completed);
            assert_eq!(
                fresh.conversation,
                Some(NativeConversationContinuityV1::New)
            );
            assert_eq!(text_output(&fresh).unwrap().0, "fresh on thread-2");
            restarted.shutdown();
            drop(restarted);
            let mut again = NativeAgentServiceV1::with_paths(paths).unwrap();
            let continued = run_local(&mut again, &agent, &workspace, "next", true);
            assert_eq!(text_output(&continued).unwrap().0, "next on thread-2");
            again.shutdown();
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn a_new_conversation_replaces_the_live_one_without_touching_other_owners() {
        let (root, workspace, agent) = conversation_codex();
        let mut service = NativeAgentServiceV1::with_paths(durable_paths(&root)).unwrap();
        run_local(&mut service, &agent, &workspace, "one", true);
        let peer = start_peer_codex_task(
            &mut service,
            &agent,
            "room-a",
            "host:mac",
            "task-peer",
            &workspace,
            "peer",
            true,
        )
        .unwrap();
        wait_for_terminal(&service, &peer.task_id);
        wait_for_workspace_release(&service, &workspace);
        let fresh = run_local(&mut service, &agent, &workspace, "two", false);
        assert_eq!(text_output(&fresh).unwrap().0, "two on thread-3");
        let peer_again = start_peer_codex_task(
            &mut service,
            &agent,
            "room-a",
            "host:mac",
            "task-peer-2",
            &workspace,
            "back",
            true,
        )
        .unwrap();
        let peer_again = wait_for_terminal(&service, &peer_again.task_id);
        assert_eq!(text_output(&peer_again).unwrap().0, "back on thread-2");
        assert!(peer_again.session_reused);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_turn_at_restart_blocks_follow_ups_and_is_never_rerun() {
        let (root, workspace, agent) = conversation_codex();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        fs::write(root.join("hold-turn"), b"").unwrap();
        start_peer_codex_task(
            &mut service,
            &agent,
            "room-a",
            "host:mac",
            "task-inflight",
            &workspace,
            "inflight",
            true,
        )
        .unwrap();
        for _ in 0..500 {
            if native_log(&root)
                .iter()
                .any(|line| line.starts_with("turn"))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Pastey stops while the turn is in flight.
        service.shutdown();
        drop(service);
        let _ = fs::remove_file(root.join("hold-turn"));

        let mut restarted = NativeAgentServiceV1::with_paths(paths).unwrap();
        let restored = restarted.task_status("task-inflight").unwrap();
        assert_eq!(restored.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            restored.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        // The unresolved turn still owns the workspace: no follow-up overlaps it.
        assert!(start_peer_codex_task(
            &mut restarted,
            &agent,
            "room-a",
            "host:mac",
            "task-follow",
            &workspace,
            "follow",
            true,
        )
        .is_err());
        // Explicit stop resolves it; then the conversation continues, and
        // the interrupted prompt is never sent again.
        restarted
            .cancel_bridge_task("room-a", "task-inflight")
            .unwrap();
        let follow = start_peer_codex_task(
            &mut restarted,
            &agent,
            "room-a",
            "host:mac",
            "task-follow-2",
            &workspace,
            "follow",
            true,
        )
        .unwrap();
        let follow = wait_for_terminal(&restarted, &follow.task_id);
        assert_eq!(
            follow.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert_eq!(text_output(&follow).unwrap().0, "follow on thread-1");
        let log = native_log(&root);
        assert_eq!(
            log.iter().filter(|line| line.contains("inflight")).count(),
            1,
            "{log:?}"
        );
        assert_eq!(
            log.iter().filter(|line| line.starts_with("start")).count(),
            1
        );
        restarted.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    /// Stop after an executor restart revokes Pastey's authority; it is not
    /// evidence that native work stopped, and it must say so.
    #[test]
    fn stop_after_restart_revokes_authority_without_inventing_a_native_outcome() {
        let (root, workspace, agent) = conversation_codex();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        fs::write(root.join("hold-turn"), b"").unwrap();
        start_peer_codex_task(
            &mut service,
            &agent,
            "room-s",
            "host:mac",
            "task-stop",
            &workspace,
            "inflight",
            true,
        )
        .unwrap();
        for _ in 0..500 {
            if native_log(&root)
                .iter()
                .any(|line| line.starts_with("turn"))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // While the observer is live, Stop never frees the workspace.
        let live = service.cancel_bridge_task("room-s", "task-stop").unwrap();
        assert_eq!(live.state, NativeAgentTaskStateV1::Cancelled);
        assert!(service
            .workspace_occupant(&workspace.canonicalize().unwrap())
            .is_some());
        service.shutdown();
        drop(service);
        let _ = fs::remove_file(root.join("hold-turn"));

        // A second task is in flight when Pastey stops abruptly.
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        fs::write(root.join("hold-turn"), b"").unwrap();
        start_peer_codex_task(
            &mut service,
            &agent,
            "room-s",
            "host:mac",
            "task-restored",
            &workspace,
            "second",
            true,
        )
        .unwrap();
        for _ in 0..500 {
            if native_log(&root)
                .iter()
                .any(|line| line.ends_with("second"))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        service.shutdown();
        drop(service);
        let _ = fs::remove_file(root.join("hold-turn"));
        let mut restarted = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let restored = restarted.task_status("task-restored").unwrap();
        assert_eq!(
            restored.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        let stopped = restarted
            .cancel_bridge_task("room-s", "task-restored")
            .unwrap();
        // Pastey's authority is revoked; no native completion, failure or
        // interruption is claimed, and the stop itself is reported uncertain.
        assert_eq!(stopped.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            stopped.code.as_deref(),
            Some("native_agent_cancel_delivery_uncertain")
        );
        assert_eq!(stopped.output, None);
        // The fact survives restart and is what reconciliation reports.
        let again = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert_eq!(again.task_status("task-restored").unwrap(), stopped);
        let fact = again
            .reconciliation_fact("room-s", "task-restored", None, "executor", "host:mac")
            .unwrap();
        assert_eq!(fact.task_state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            fact.code.as_deref(),
            Some("native_agent_cancel_delivery_uncertain")
        );
        // Nothing was retried: each prompt reached Codex exactly once.
        let log = native_log(&root);
        assert_eq!(
            log.iter().filter(|line| line.ends_with(" second")).count(),
            1,
            "{log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|line| line.ends_with(" inflight"))
                .count(),
            1,
            "{log:?}"
        );
        restarted.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_follow_ups_are_exclusive_and_each_runs_once() {
        let (root, workspace, agent) = conversation_codex();
        let mut service = NativeAgentServiceV1::with_paths(durable_paths(&root)).unwrap();
        run_local(&mut service, &agent, &workspace, "opening", true);
        fs::write(root.join("hold-turn"), b"").unwrap();
        let first = service
            .start_codex_task_with_executable_id_and_resume(
                &agent,
                "native-agent:follow-1",
                &workspace.canonicalize().unwrap(),
                "followone",
                true,
            )
            .unwrap();
        // A second follow-up of the same conversation, and a new
        // conversation, both wait for the workspace.
        for resume in [true, false] {
            assert!(service
                .start_codex_task_with_executable_id_and_resume(
                    &agent,
                    &format!("native-agent:follow-2-{resume}"),
                    &workspace.canonicalize().unwrap(),
                    "followtwo",
                    resume,
                )
                .is_err());
        }
        // Replaying the first task's identity starts nothing new.
        assert_eq!(
            service
                .start_codex_task_with_executable_id_and_resume(
                    &agent,
                    "native-agent:follow-1",
                    &workspace.canonicalize().unwrap(),
                    "followone",
                    true,
                )
                .unwrap()
                .task_id,
            first.task_id
        );
        fs::remove_file(root.join("hold-turn")).unwrap();
        wait_for_terminal(&service, &first.task_id);
        wait_for_workspace_release(&service, &workspace);
        let second = run_local(&mut service, &agent, &workspace, "followtwo", true);
        assert_eq!(text_output(&second).unwrap().0, "followtwo on thread-1");
        let log = native_log(&root);
        assert_eq!(
            log.iter().filter(|line| line.starts_with("turn")).count(),
            3,
            "{log:?}"
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancel_while_a_conversation_reopens_sends_no_turn_and_keeps_it() {
        let (root, workspace, agent) = conversation_codex();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        run_local(&mut service, &agent, &workspace, "first", true);
        service.shutdown();
        drop(service);
        fs::write(root.join("hold-resume"), b"").unwrap();
        let mut restarted = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let started = restarted
            .start_codex_task_with_executable_id_and_resume(
                &agent,
                "native-agent:reopening",
                &workspace.canonicalize().unwrap(),
                "cancelled",
                true,
            )
            .unwrap();
        for _ in 0..500 {
            if native_log(&root)
                .iter()
                .any(|line| line.starts_with("resume"))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            restarted.cancel_task(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        fs::remove_file(root.join("hold-resume")).unwrap();
        wait_for_workspace_release(&restarted, &workspace);
        let after = restarted.task_status(&started.task_id).unwrap();
        assert_eq!(after.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(after.output, None);
        let next = run_local(&mut restarted, &agent, &workspace, "next", true);
        assert_eq!(text_output(&next).unwrap().0, "next on thread-1");
        assert!(!native_log(&root)
            .iter()
            .any(|line| line.contains("cancelled")));
        restarted.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn burn_forgets_a_bridges_conversations_and_stale_records_cannot_return() {
        let (root, workspace, agent) = conversation_codex();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let started = start_peer_codex_task(
            &mut service,
            &agent,
            "room-burn",
            "host:mac",
            "task-burn",
            &workspace,
            "before",
            true,
        )
        .unwrap();
        wait_for_terminal(&service, &started.task_id);
        wait_for_workspace_release(&service, &workspace);
        let kept = start_peer_codex_task(
            &mut service,
            &agent,
            "room-kept",
            "host:mac",
            "task-kept",
            &workspace,
            "kept",
            true,
        );
        wait_for_terminal(&service, &kept.unwrap().task_id);
        let binding = |bridge: &str| {
            NativeConversationBindingV1::new(
                NativeConversationOwnerV1::Peer {
                    bridge_id: bridge.into(),
                    peer_host_ref: "host:mac".into(),
                },
                CODEX_CAPABILITY_ID,
                &workspace.canonicalize().unwrap().to_string_lossy(),
            )
            .unwrap()
        };
        assert!(binding("room-burn")
            .saved_continuation(&paths)
            .unwrap()
            .is_some());
        assert_eq!(service.codex.session_count(), 2);

        service.purge_bridge_authority("room-burn").unwrap();
        assert!(binding("room-burn")
            .saved_continuation(&paths)
            .unwrap()
            .is_none());
        assert!(binding("room-kept")
            .saved_continuation(&paths)
            .unwrap()
            .is_some());
        assert_eq!(service.codex.session_count(), 1);
        // The burned Bridge can start nothing, so nothing reopens.
        assert!(start_peer_codex_task(
            &mut service,
            &agent,
            "room-burn",
            "host:mac",
            "task-burn-2",
            &workspace,
            "after",
            true,
        )
        .is_err());

        // A late write after the durable Burn cutoff is refused, and a
        // record that survived somehow is never read back.
        let connection = rusqlite::Connection::open(&paths.db_path).unwrap();
        connection
            .execute(
                "INSERT INTO burned_bridges (room_id, burned_at) VALUES ('room-burn', 1)",
                [],
            )
            .unwrap();
        assert!(!binding("room-burn")
            .save(
                &paths,
                serde_json::from_value::<OpaqueCapabilityPayloadV1>(
                    json!({"threadId": "thread-1"})
                )
                .unwrap()
            )
            .unwrap());
        connection
            .execute(
                "INSERT INTO native_agent_conversations (conversation_key, bridge_id, record_json, updated_at)
                 VALUES (?1, 'room-burn', ?2, 1)",
                rusqlite::params![
                    binding("room-burn").key,
                    serde_json::to_string(&NativeConversationRecordV1 {
                        schema_version: NATIVE_CONVERSATION_SCHEMA.into(),
                        owner: binding("room-burn").owner,
                        capability_id: CODEX_CAPABILITY_ID.into(),
                        scope_digest: binding("room-burn").scope_digest,
                        continuation: serde_json::from_value::<OpaqueCapabilityPayloadV1>(json!({"threadId": "thread-1"})).unwrap(),
                    })
                    .unwrap()
                ],
            )
            .unwrap();
        assert!(binding("room-burn")
            .saved_continuation(&paths)
            .unwrap()
            .is_none());
        // Finalization removes it for good.
        purge_durable_bridge_state(&paths, "room-burn").unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM native_agent_conversations WHERE bridge_id = 'room-burn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_sessions_and_task_slots_stay_bounded_and_evicted_conversations_reopen() {
        let (root, workspace, agent) = conversation_codex();
        let mut service = NativeAgentServiceV1::with_paths(durable_paths(&root)).unwrap();
        for turn in 0..6 {
            run_local(
                &mut service,
                &agent,
                &workspace,
                &format!("turn{turn}"),
                true,
            );
        }
        // Many tasks in one conversation keep only what Burn can reach.
        assert!(
            service.codex.task_count() <= 2,
            "{}",
            service.codex.task_count()
        );
        let workspaces = (0..5)
            .map(|index| {
                let path = root.join(format!("ws{index}"));
                fs::create_dir(&path).unwrap();
                run_local(&mut service, &agent, &path, "open", true);
                path
            })
            .collect::<Vec<_>>();
        assert!(
            service.codex.session_count() <= 4,
            "{}",
            service.codex.session_count()
        );
        // The least recently used conversation was evicted, not forgotten.
        let reopened = run_local(&mut service, &agent, &workspace, "back", true);
        assert_eq!(
            reopened.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert_eq!(text_output(&reopened).unwrap().0, "back on thread-1");
        assert!(native_log(&root).contains(&"resume thread-1".to_owned()));
        assert!(
            service.codex.task_count() <= 6,
            "{}",
            service.codex.task_count()
        );
        drop(workspaces);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_requesters_see_only_whether_the_conversation_continued() {
        let mut requester = NativeAgentServiceV1::default();
        requester
            .queue_bridge_remote_task("room-c", "task-c", "executor-host", r"C:\repz", "go")
            .unwrap();
        let status = NativeAgentTaskStatusV1 {
            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
            task_id: "task-c".into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: "repz".into(),
            session_reused: true,
            state: NativeAgentTaskStateV1::Running,
            result: None,
            code: None,
            output: None,
            conversation: Some(NativeConversationContinuityV1::Continued),
            native_decisions: Vec::new(),
        };
        let wire = serde_json::to_value(NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-c".into(),
            executing_host_ref: "executor-host".into(),
            status,
        })
        .unwrap();
        assert_eq!(wire["status"]["conversation"], "continued");
        let message: NativeAgentStatusV1 = serde_json::from_value(wire).unwrap();
        validate_status(&message).unwrap();
        let recorded = requester
            .record_bridge_remote_status("room-c", message)
            .unwrap();
        assert_eq!(
            recorded.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
    }

    /// One Windows Host (this service) and one requesting Mac, with the Mac's
    /// real identity proven in one exact session and a REPZ-like workspace
    /// outside the OS temp folder (which on Windows is application data).
    struct UnattendedFixture {
        root: PathBuf,
        agent: PathBuf,
        paths: crate::storage::AppPaths,
        workspace: PathBuf,
        mac: crate::device_identity::DeviceIdentity,
        windows_host: crate::host_identity::HostRef,
        service: NativeAgentServiceV1,
    }

    const GRANT_BRIDGE: &str = "room-unattended";

    impl Drop for UnattendedFixture {
        fn drop(&mut self) {
            self.service.shutdown();
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_dir_all(self.workspace.parent().unwrap());
        }
    }

    fn unattended_fixture() -> UnattendedFixture {
        let (root, _, agent) = conversation_codex();
        let paths = durable_paths(&root);
        crate::storage::create_room(
            &paths,
            &[5u8; 32],
            "12345678",
            15,
            crate::models::LocalRole::Creator,
            Some(GRANT_BRIDGE.into()),
            None,
        )
        .unwrap();
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("pastey-test-grants")
            .join(Uuid::new_v4().to_string())
            .join("REPZ");
        fs::create_dir_all(&workspace).unwrap();
        let mac_paths = durable_paths(&root.join("mac"));
        let mac = crate::device_identity::load_or_create(&mac_paths, &[6u8; 32]).unwrap();
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service.codex.use_executable(&agent);
        UnattendedFixture {
            root,
            agent,
            paths,
            workspace: workspace.canonicalize().unwrap(),
            mac,
            windows_host: crate::host_identity::HostRef::from_device_id("windows").unwrap(),
            service,
        }
    }

    impl UnattendedFixture {
        /// Both sides' bindings of one session.
        fn session(
            &self,
            bridge: &str,
            mac_device: &str,
            session: &str,
        ) -> (HostSessionBinding, HostSessionBinding) {
            let mac_host = crate::host_identity::HostRef::from_device_id(mac_device).unwrap();
            let windows = HostSessionBinding::new(
                bridge,
                self.windows_host.clone(),
                mac_host.clone(),
                &format!("win-{session}"),
                &format!("mac-{session}"),
                &format!("route-{session}"),
                crate::storage::now_ts() + 3_600,
            )
            .unwrap();
            let mac = HostSessionBinding::new(
                bridge,
                mac_host,
                self.windows_host.clone(),
                &format!("mac-{session}"),
                &format!("win-{session}"),
                &format!("route-back-{session}"),
                crate::storage::now_ts() + 3_600,
            )
            .unwrap();
            (windows, mac)
        }

        /// The Mac proves its identity in the session; Windows verifies it.
        fn proven(
            &self,
            bridge: &str,
            mac_device: &str,
            session: &str,
        ) -> (
            HostSessionBinding,
            crate::device_identity::VerifiedPeerDevice,
        ) {
            let (windows, mac) = self.session(bridge, mac_device, session);
            let proof = self.mac.attest(&mac, &Uuid::new_v4().to_string()).unwrap();
            let verified = crate::device_identity::verify_attestation(&proof, &windows).unwrap();
            (windows, verified)
        }

        fn trust_and_grant(
            &self,
            device: &crate::device_identity::VerifiedPeerDevice,
            hours: i64,
            runs: i64,
        ) -> String {
            crate::unattended_access::trust_seen_device(
                &self.paths,
                &[device.clone()],
                &device.fingerprint,
                "School Mac",
            )
            .unwrap();
            crate::unattended_access::create_grant(
                &self.paths,
                &self.windows_host,
                &[device.clone()],
                &crate::unattended_access::CreateUnattendedGrantV1 {
                    device_fingerprint: device.fingerprint.clone(),
                    room_id: device.binding.bridge_id.clone(),
                    workspace_path: self.workspace.to_string_lossy().into_owned(),
                    duration_hours: hours,
                    max_runs: runs,
                },
                crate::storage::now_ts(),
            )
            .unwrap()
            .grant_id
        }

        fn invoke(
            &mut self,
            binding: &HostSessionBinding,
            device: Option<&crate::device_identity::VerifiedPeerDevice>,
            task_id: &str,
            workspace: &Path,
            task: &str,
        ) -> AppResult<NativeAgentTaskStatusV1> {
            let request = NativeInvocationRequestV1 {
                task_id: task_id.into(),
                target_host_ref: self.windows_host.as_str().into(),
                agent_capability: CODEX_CAPABILITY_ID.into(),
                input: capability::codex::codex_input(workspace, task, true).unwrap(),
            };
            self.service.receive_bridge_invocation(
                binding,
                &request,
                crate::storage::now_ts(),
                device,
            )
        }

        fn runs(&self, grant_id: &str) -> i64 {
            crate::storage::list_unattended_grants(&self.paths)
                .unwrap()
                .into_iter()
                .find(|grant| grant.grant_id == grant_id)
                .unwrap()
                .uses
        }

        fn launches(&self) -> usize {
            invocation_count(&self.root, "app-server --stdio")
        }

        fn finish(&self, task_id: &str) -> NativeAgentTaskStatusV1 {
            let terminal = wait_for_terminal(&self.service, task_id);
            wait_for_workspace_release(&self.service, &self.workspace);
            terminal
        }
    }

    fn assert_waits_for_review(status: &NativeAgentTaskStatusV1) {
        assert_eq!(status.state, NativeAgentTaskStateV1::Queued, "{status:?}");
        assert_eq!(
            status.code.as_deref(),
            Some(admission::REVIEW_REQUIRED_CODE)
        );
    }

    #[test]
    fn an_unattended_grant_runs_only_its_trusted_scope_and_continues_its_conversation() {
        let mut h = unattended_fixture();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 24, 2);
        let workspace = h.workspace.clone();
        // No Review on Windows: it runs at once and returns Codex's answer.
        let first = h
            .invoke(&binding, Some(&device), "task-u1", &workspace, "render")
            .unwrap();
        assert_eq!(first.state, NativeAgentTaskStateV1::Running);
        let first = h.finish("task-u1");
        assert_eq!(first.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(text_output(&first).unwrap().0, "render on thread-1");
        assert_eq!(h.runs(&grant), 1);
        // A follow-up continues the same Pastey-managed conversation.
        let second = h
            .invoke(&binding, Some(&device), "task-u2", &workspace, "again")
            .unwrap();
        assert_eq!(second.state, NativeAgentTaskStateV1::Running);
        let second = h.finish("task-u2");
        assert_eq!(
            second.conversation,
            Some(NativeConversationContinuityV1::Continued)
        );
        assert_eq!(text_output(&second).unwrap().0, "again on thread-1");
        // The use limit holds; then the ordinary Review applies again, and
        // its Accept still works.
        let third = h
            .invoke(&binding, Some(&device), "task-u3", &workspace, "more")
            .unwrap();
        assert_waits_for_review(&third);
        assert_eq!(h.runs(&grant), 2);
        let accepted = h
            .service
            .accept_invocation_review("task-u3", Some(&binding), crate::storage::now_ts())
            .unwrap();
        assert_eq!(accepted.state, NativeAgentTaskStateV1::Running);
        h.finish("task-u3");
        // A grant never shares folders for Find file.
        assert!(
            crate::file_candidates::list_authorized_search_folders(&h.paths)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn wrong_device_session_bridge_peer_host_or_workspace_waits_for_review() {
        let mut h = unattended_fixture();
        crate::storage::create_room(
            &h.paths,
            &[5u8; 32],
            "87654321",
            15,
            crate::models::LocalRole::Creator,
            Some("room-other".into()),
            None,
        )
        .unwrap();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 24, 10);
        let workspace = h.workspace.clone();
        let sibling = workspace.parent().unwrap().join("Other");
        fs::create_dir_all(&sibling).unwrap();
        let mut case = 0;
        let mut expect_review = |h: &mut UnattendedFixture,
                                 binding: &HostSessionBinding,
                                 device: Option<&crate::device_identity::VerifiedPeerDevice>,
                                 workspace: &Path| {
            case += 1;
            let status = h
                .invoke(binding, device, &format!("task-n{case}"), workspace, "x")
                .unwrap();
            assert_waits_for_review(&status);
        };
        // No identity proof at all.
        expect_review(&mut h, &binding, None, &workspace);
        // Another workspace of the same Host.
        expect_review(&mut h, &binding, Some(&device), &sibling);
        // A proof from an earlier session presented in a later one.
        let (later, _) = h.proven(GRANT_BRIDGE, "mac", "s2");
        expect_review(&mut h, &later, Some(&device), &workspace);
        // The same key, proven over another Bridge.
        let (other_bridge, other_device) = h.proven("room-other", "mac", "s3");
        expect_review(&mut h, &other_bridge, Some(&other_device), &workspace);
        // The same key, now claiming to be another Host on this Bridge.
        let (other_peer, other_peer_device) = h.proven(GRANT_BRIDGE, "imposter", "s4");
        expect_review(&mut h, &other_peer, Some(&other_peer_device), &workspace);
        // A different, untrusted key in the same session.
        let stranger_paths = durable_paths(&h.root.join("stranger"));
        let stranger = crate::device_identity::load_or_create(&stranger_paths, &[7u8; 32]).unwrap();
        let (windows_side, mac_side) = h.session(GRANT_BRIDGE, "mac", "s5");
        let stranger_device = crate::device_identity::verify_attestation(
            &stranger
                .attest(&mac_side, &Uuid::new_v4().to_string())
                .unwrap(),
            &windows_side,
        )
        .unwrap();
        expect_review(&mut h, &windows_side, Some(&stranger_device), &workspace);
        // Nothing ran, nothing was spent.
        assert_eq!(h.launches(), 0);
        assert_eq!(h.runs(&grant), 0);
        // Another executing Host never matches a grant made here.
        let foreign = crate::storage::candidate_unattended_grants(
            &h.paths,
            &device.fingerprint,
            &base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                device.public_key,
            ),
            GRANT_BRIDGE,
            binding.peer_host_ref.as_str(),
            crate::host_identity::HostRef::from_device_id("another-windows")
                .unwrap()
                .as_str(),
            CODEX_CAPABILITY_ID,
            crate::storage::now_ts(),
        )
        .unwrap();
        assert!(foreign.is_empty());
    }

    #[test]
    fn expiry_revocation_and_device_revocation_end_unattended_runs() {
        let mut h = unattended_fixture();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 1, 10);
        let workspace = h.workspace.clone();
        let expired_at = crate::storage::now_ts() + 3_600;
        assert!(crate::storage::candidate_unattended_grants(
            &h.paths,
            &device.fingerprint,
            &base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                device.public_key
            ),
            GRANT_BRIDGE,
            binding.peer_host_ref.as_str(),
            h.windows_host.as_str(),
            CODEX_CAPABILITY_ID,
            expired_at,
        )
        .unwrap()
        .is_empty());
        assert!(!crate::storage::consume_unattended_grant(
            &h.paths,
            &grant,
            "task-late",
            expired_at
        )
        .unwrap());
        crate::unattended_access::revoke_grant(&h.paths, &grant).unwrap();
        assert_waits_for_review(
            &h.invoke(&binding, Some(&device), "task-r1", &workspace, "x")
                .unwrap(),
        );
        // A requester cannot renew or reactivate: only a new Host-local grant
        // works, and revoking the device ends every grant it holds.
        let renewed = h.trust_and_grant(&device, 24, 10);
        crate::unattended_access::revoke_device(&h.paths, &device.fingerprint).unwrap();
        assert_waits_for_review(
            &h.invoke(&binding, Some(&device), "task-r2", &workspace, "x")
                .unwrap(),
        );
        let states = crate::unattended_access::grants(&h.paths, crate::storage::now_ts()).unwrap();
        assert!(states
            .iter()
            .all(|view| view.state == crate::unattended_access::GrantStateV1::Revoked));
        assert_eq!(h.runs(&renewed), 0);
        // A grant needs a device proven on that Bridge now.
        assert!(crate::unattended_access::create_grant(
            &h.paths,
            &h.windows_host,
            &[],
            &crate::unattended_access::CreateUnattendedGrantV1 {
                device_fingerprint: device.fingerprint.clone(),
                room_id: GRANT_BRIDGE.into(),
                workspace_path: workspace.to_string_lossy().into_owned(),
                duration_hours: 1,
                max_runs: 1,
            },
            crate::storage::now_ts(),
        )
        .is_err());
        assert_eq!(h.launches(), 0);
    }

    #[test]
    fn duplicate_and_concurrent_requests_never_spend_one_use_twice() {
        let mut h = unattended_fixture();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 24, 3);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-d1", &workspace, "once")
            .unwrap();
        // The same task delivered again answers with the same task.
        let replay = h
            .invoke(&binding, Some(&device), "task-d1", &workspace, "once")
            .unwrap();
        assert_eq!(replay.task_id, "task-d1");
        h.finish("task-d1");
        assert_eq!(h.runs(&grant), 1);
        assert!(!crate::storage::consume_unattended_grant(
            &h.paths,
            &grant,
            "task-d1",
            crate::storage::now_ts()
        )
        .unwrap());
        // Many simultaneous requests for the last two uses: exactly two win.
        let winners = (0..8)
            .map(|index| {
                let paths = h.paths.clone();
                let grant = grant.clone();
                std::thread::spawn(move || {
                    crate::storage::consume_unattended_grant(
                        &paths,
                        &grant,
                        &format!("task-c{index}"),
                        crate::storage::now_ts(),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 2);
        assert_eq!(h.runs(&grant), 3);
        let turns = native_log(&h.root)
            .iter()
            .filter(|line| line.starts_with("turn"))
            .count();
        assert_eq!(turns, 1);
    }

    #[test]
    fn a_replaced_or_aliased_workspace_never_runs_unattended() {
        let mut h = unattended_fixture();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 24, 5);
        let workspace = h.workspace.clone();
        let elsewhere = workspace.parent().unwrap().join("Elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        #[cfg(unix)]
        {
            // An alias that resolves into the workspace is the same workspace,
            // but one that resolves elsewhere is not.
            let alias = workspace.parent().unwrap().join("alias-to-elsewhere");
            std::os::unix::fs::symlink(&elsewhere, &alias).unwrap();
            assert_waits_for_review(
                &h.invoke(&binding, Some(&device), "task-a1", &alias, "x")
                    .unwrap(),
            );
            // The granted folder itself replaced by a link to elsewhere.
            let moved = workspace.parent().unwrap().join("REPZ-moved");
            fs::rename(&workspace, &moved).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &workspace).unwrap();
            assert_waits_for_review(
                &h.invoke(&binding, Some(&device), "task-a2", &workspace, "x")
                    .unwrap(),
            );
            let view =
                crate::unattended_access::grants(&h.paths, crate::storage::now_ts()).unwrap();
            assert_eq!(
                view[0].state,
                crate::unattended_access::GrantStateV1::Unavailable
            );
            fs::remove_file(&workspace).unwrap();
            fs::rename(&moved, &workspace).unwrap();
        }
        // A start whose adapter-resolved workspace differs from the admitted
        // one is refused before anything runs.
        let input = capability::codex::codex_input(&elsewhere, "x", true).unwrap();
        assert!(h
            .service
            .start_invocation_in_scope(
                CODEX_CAPABILITY_ID,
                "task-a3",
                &input,
                None,
                None,
                Some(&workspace)
            )
            .is_err());
        assert_eq!(h.runs(&grant), 0);
        assert_eq!(h.launches(), 0);
    }

    #[test]
    fn grants_survive_restart_with_a_fresh_proof_and_end_with_burn() {
        let mut h = unattended_fixture();
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        let grant = h.trust_and_grant(&device, 24, 5);
        let workspace = h.workspace.clone();
        h.service.shutdown();
        h.service = NativeAgentServiceV1::with_paths(h.paths.clone()).unwrap();
        h.service.codex.use_executable(&h.agent);
        // After restart, a new session and a fresh proof are needed.
        let (fresh, fresh_device) = h.proven(GRANT_BRIDGE, "mac", "after-restart");
        assert_eq!(
            h.invoke(&fresh, Some(&fresh_device), "task-b1", &workspace, "x")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Running
        );
        h.finish("task-b1");
        // The pre-restart proof does not carry over to the new session.
        assert_waits_for_review(
            &h.invoke(&fresh, Some(&device), "task-b0", &workspace, "x")
                .unwrap(),
        );
        let _ = &binding;
        // Burn: the cutoff ends the grant at once; finalization deletes it.
        crate::storage::cut_off_bridge_authority(&h.paths, GRANT_BRIDGE).unwrap();
        assert!(!crate::storage::unattended_grant_is_active(
            &h.paths,
            &grant,
            crate::storage::now_ts()
        )
        .unwrap());
        h.service.purge_bridge_authority(GRANT_BRIDGE).unwrap();
        assert!(h
            .invoke(&fresh, Some(&fresh_device), "task-b2", &workspace, "x")
            .is_err());
        crate::storage::finalize_burned_room(&h.paths, GRANT_BRIDGE, &h.paths.inbox_dir).unwrap();
        assert!(crate::storage::list_unattended_grants(&h.paths)
            .unwrap()
            .is_empty());
        assert_eq!(h.runs_or_zero(&grant), 0);
    }

    impl UnattendedFixture {
        fn runs_or_zero(&self, grant_id: &str) -> i64 {
            crate::storage::list_unattended_grants(&self.paths)
                .unwrap()
                .into_iter()
                .find(|grant| grant.grant_id == grant_id)
                .map_or(0, |grant| grant.uses)
        }
    }

    #[test]
    fn an_unattended_run_never_answers_codexs_own_approvals_and_stays_cancellable() {
        let (approval_root, _, approval_agent) = scripted_codex(&format!(
            r#"{SCRIPTED_OPEN}    *'"id":3,'*)
      {{ACK}}
      printf '%s\n' '{{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{{"threadId":"native-thread","turnId":"native-turn","itemId":"cmd-1","command":"rm -rf Saved"}}}}'
      ;;
    *'"id":"approval-1"'*) printf '%s\n' "$line" > "$PWD/approval-answered" ;;
    *'turn/interrupt'*)
      printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"native-thread","turn":{{"id":"native-turn","items":[],"status":"interrupted","error":null}}}}}}'
      ;;
"#
        ));
        let mut h = unattended_fixture();
        h.service.codex.use_executable(&approval_agent);
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        h.trust_and_grant(&device, 24, 5);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-p1", &workspace, "risky")
            .unwrap();
        wait_for_code(&h.service, "task-p1", Some(AWAITING_NATIVE_DECISION_CODE));
        thread::sleep(Duration::from_millis(200));
        assert!(!workspace.join("approval-answered").exists());
        assert_eq!(
            h.service.task_status("task-p1").unwrap().state,
            NativeAgentTaskStateV1::Running
        );
        // The requester's Stop still works over the same Bridge.
        assert_eq!(
            h.service
                .cancel_bridge_task(GRANT_BRIDGE, "task-p1")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Cancelled
        );
        wait_for_workspace_release(&h.service, &workspace);
        assert!(!workspace.join("approval-answered").exists());
        let _ = fs::remove_dir_all(approval_root);
    }

    /// Codex raising a command approval and a file-change approval (whose
    /// diff its `item/started` carried) in one turn. Each answer is recorded
    /// verbatim in `answers`, and Codex then reports that request resolved.
    /// Its waits use the fixture's `fixture_alive` guard, so a stopped task
    /// leaves no fixture process behind.
    const APPROVAL_TURN: &str = r#"    *'"id":3,'*)
      {ACK}
      printf '%s\n' '{"method":"item/started","params":{"threadId":"native-thread","turnId":"native-turn","startedAtMs":1,"item":{"type":"fileChange","id":"patch-1","status":"inProgress","changes":[{"path":"src/lib.rs","kind":{"type":"update","move_path":null},"diff":"-a\n+b\n"}]}}}'
      printf '%s\n' '{"id":"approval-cmd","method":"item/commandExecution/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"cmd-1","startedAtMs":1,"command":"cargo test","cwd":"/repo","reason":"Run the tests <b>now</b>"}}'
      printf '%s\n' '{"id":7,"method":"item/fileChange/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"patch-1","startedAtMs":1}}'
      (
        while [ ! -f "$PWD/patch-updated" ]; do [ -f "$PWD/finish-turn" ] && break; fixture_alive || exit 0; sleep 0.01; done
        if [ -f "$PWD/patch-updated" ]; then
          printf '%s\n' '{"method":"item/fileChange/patchUpdated","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"patch-1","changes":[{"path":"src/main.rs","kind":{"type":"delete"},"diff":""}]}}'
          touch "$PWD/patch-update-sent"
        fi
        while [ ! -f "$PWD/finish-turn" ]; do fixture_alive || exit 0; sleep 0.01; done
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"native-thread","turnId":"native-turn","item":{"type":"agentMessage","id":"m","text":"Done after review."}}}'
        {COMPLETED}
      ) &
      ;;
    *'"id":"approval-cmd"'*)
      printf '%s\n' "$line" >> "$PWD/answers"
      printf '%s\n' '{"method":"serverRequest/resolved","params":{"threadId":"native-thread","requestId":"approval-cmd"}}'
      ;;
    *'"id":7,'*)
      printf '%s\n' "$line" >> "$PWD/answers"
      printf '%s\n' '{"method":"serverRequest/resolved","params":{"threadId":"native-thread","requestId":7}}'
      ;;
    *'turn/interrupt'*)
      printf '%s\n' '{"method":"serverRequest/resolved","params":{"threadId":"native-thread","requestId":"approval-cmd"}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"interrupted","error":null}}}'
      ;;
"#;

    /// The unattended fixture's Host running `turn` on its Codex, with the
    /// requesting Mac proven in session `s1` and holding a grant.
    fn approval_fixture(
        turn: &str,
    ) -> (
        UnattendedFixture,
        PathBuf,
        HostSessionBinding,
        crate::device_identity::VerifiedPeerDevice,
    ) {
        let (script_root, _, agent) = scripted_codex(&format!("{SCRIPTED_OPEN}{turn}"));
        let mut h = unattended_fixture();
        h.service.codex.use_executable(&agent);
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s1");
        h.trust_and_grant(&device, 24, 5);
        (h, script_root, binding, device)
    }

    fn wait_for_reviews(
        service: &NativeAgentServiceV1,
        task_id: &str,
        count: usize,
    ) -> Vec<NativeDecisionReviewV1> {
        for _ in 0..500 {
            let reviews = service.task_status(task_id).unwrap().native_decisions;
            if reviews.len() >= count {
                return reviews;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "{count} reviews never appeared: {:?}",
            service.task_status(task_id)
        )
    }

    fn wait_for_review_state(
        service: &NativeAgentServiceV1,
        task_id: &str,
        review_ref: &str,
        state: NativeDecisionStateV1,
    ) -> NativeDecisionReviewV1 {
        for _ in 0..500 {
            if let Some(review) = service
                .task_status(task_id)
                .unwrap()
                .native_decisions
                .into_iter()
                .find(|review| review.review_ref == review_ref && review.state == state)
            {
                return review;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "review never became {state:?}: {:?}",
            service.task_status(task_id)
        )
    }

    fn answers(workspace: &Path) -> Vec<String> {
        fs::read_to_string(workspace.join("answers"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn review_of_kind(
        reviews: &[NativeDecisionReviewV1],
        kind: NativeDecisionKindV1,
    ) -> NativeDecisionReviewV1 {
        reviews
            .iter()
            .find(|review| review.kind == kind)
            .unwrap()
            .clone()
    }

    impl UnattendedFixture {
        fn decide(
            &mut self,
            binding: &HostSessionBinding,
            device: Option<&crate::device_identity::VerifiedPeerDevice>,
            task_id: &str,
            review: &NativeDecisionReviewV1,
            decision: NativeDecisionV1,
        ) -> AppResult<NativeAgentTaskStatusV1> {
            self.decide_at(
                binding,
                device,
                task_id,
                review,
                decision,
                crate::storage::now_ts(),
            )
        }

        fn decide_at(
            &mut self,
            binding: &HostSessionBinding,
            device: Option<&crate::device_identity::VerifiedPeerDevice>,
            task_id: &str,
            review: &NativeDecisionReviewV1,
            decision: NativeDecisionV1,
            now: i64,
        ) -> AppResult<NativeAgentTaskStatusV1> {
            let request = NativeAgentDecisionV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                decision_id: Uuid::new_v4().to_string(),
                task_id: task_id.into(),
                target_host_ref: self.windows_host.as_str().into(),
                review_ref: review.review_ref.clone(),
                content_digest: review.content_digest.clone(),
                decision,
            };
            self.service
                .submit_native_decision(binding, device, &request, now)
        }
    }

    #[test]
    fn the_requesting_user_approves_a_command_once_and_declines_a_file_change() {
        let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a1", &workspace, "work")
            .unwrap();
        let reviews = wait_for_reviews(&h.service, "task-a1", 2);
        let status = h.service.task_status("task-a1").unwrap();
        assert_eq!(status.state, NativeAgentTaskStateV1::Running);
        assert_eq!(status.code.as_deref(), Some(AWAITING_NATIVE_DECISION_CODE));
        let command = review_of_kind(&reviews, NativeDecisionKindV1::CommandExecution);
        let content = command.content.clone().unwrap();
        assert_eq!(content.command.as_deref(), Some("cargo test"));
        assert_eq!(content.cwd.as_deref(), Some("/repo"));
        assert_eq!(content.reason.as_deref(), Some("Run the tests <b>now</b>"));
        assert!(command.can_accept && command.can_decline);
        let patch = review_of_kind(&reviews, NativeDecisionKindV1::FileChange);
        let changes = patch.content.clone().unwrap().file_changes;
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "src/lib.rs");
        assert_eq!(changes[0].diff, "-a\n+b\n");
        assert!(patch.can_accept && patch.can_decline);
        // A valid status carrying them crosses the wire; native ids do not.
        let wire = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-a1".into(),
            executing_host_ref: h.windows_host.as_str().into(),
            status: status.clone(),
        };
        validate_status(&wire).unwrap();
        let encoded = serde_json::to_string(&wire).unwrap();
        for native in [
            "approval-cmd",
            "native-thread",
            "native-turn",
            "patch-1",
            "cmd-1",
        ] {
            assert!(!encoded.contains(native), "{native}");
        }

        // Review content is never written to disk.
        h.service.persist_envelope("task-a1", None).unwrap();
        let stored = crate::storage::get_native_agent_envelope(&h.paths, "task-a1")
            .unwrap()
            .unwrap();
        assert!(!stored.record_json.contains("cargo test"));
        assert!(!stored.record_json.contains("nativeDecisions"));

        // The grant that started the task answered nothing.
        thread::sleep(Duration::from_millis(150));
        assert!(answers(&workspace).is_empty());

        h.decide(
            &binding,
            Some(&device),
            "task-a1",
            &command,
            NativeDecisionV1::Accept,
        )
        .unwrap();
        // Replays and second answers never reach Codex again.
        for decision in [NativeDecisionV1::Accept, NativeDecisionV1::Decline] {
            assert!(h
                .decide(&binding, Some(&device), "task-a1", &command, decision)
                .is_err());
        }
        wait_for_review_state(
            &h.service,
            "task-a1",
            &command.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        h.decide(
            &binding,
            Some(&device),
            "task-a1",
            &patch,
            NativeDecisionV1::Decline,
        )
        .unwrap();
        let resolved = wait_for_review_state(
            &h.service,
            "task-a1",
            &patch.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        assert_eq!(resolved.submitted, Some(NativeDecisionV1::Decline));
        assert!(!resolved.can_accept && !resolved.can_decline);
        assert_eq!(
            answers(&workspace),
            [
                r#"{"id":"approval-cmd","result":{"decision":"accept"}}"#,
                r#"{"id":7,"result":{"decision":"decline"}}"#,
            ]
        );
        wait_for_code(&h.service, "task-a1", None);

        // The turn continues and its answer is captured as before.
        fs::write(workspace.join("finish-turn"), b"").unwrap();
        let terminal = wait_for_terminal(&h.service, "task-a1");
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        assert_eq!(
            text_output(&terminal),
            Some(("Done after review.".into(), false))
        );
        assert!(terminal.native_decisions.is_empty());
        wait_for_workspace_release(&h.service, &workspace);
        // Nothing is answerable after the turn ended.
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a1",
                &patch,
                NativeDecisionV1::Accept
            )
            .is_err());
        assert_eq!(answers(&workspace).len(), 2);
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn only_the_requesting_device_in_a_current_session_of_its_bridge_may_decide() {
        let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a2", &workspace, "work")
            .unwrap();
        let reviews = wait_for_reviews(&h.service, "task-a2", 2);
        let command = review_of_kind(&reviews, NativeDecisionKindV1::CommandExecution);
        let accept = NativeDecisionV1::Accept;

        // No device proof at all.
        assert!(h
            .decide(&binding, None, "task-a2", &command, accept)
            .is_err());
        // Another device key, even in the requester's own session.
        let intruder = crate::device_identity::load_or_create(
            &durable_paths(&h.root.join("intruder")),
            &[7u8; 32],
        )
        .unwrap();
        let (_, mac_side) = h.session(GRANT_BRIDGE, "mac", "s1");
        let intruder_here = crate::device_identity::verify_attestation(
            &intruder
                .attest(&mac_side, &Uuid::new_v4().to_string())
                .unwrap(),
            &binding,
        )
        .unwrap();
        assert!(h
            .decide(&binding, Some(&intruder_here), "task-a2", &command, accept)
            .is_err());
        // Another member of the same Bridge, with its own proven session.
        let (other_session, other_mac) = h.session(GRANT_BRIDGE, "other-mac", "s9");
        let other = crate::device_identity::verify_attestation(
            &intruder
                .attest(&other_mac, &Uuid::new_v4().to_string())
                .unwrap(),
            &other_session,
        )
        .unwrap();
        assert!(h
            .decide(&other_session, Some(&other), "task-a2", &command, accept)
            .is_err());
        // The requesting device over another Bridge.
        let (elsewhere, elsewhere_device) = h.proven("room-other", "mac", "s3");
        assert!(h
            .decide(
                &elsewhere,
                Some(&elsewhere_device),
                "task-a2",
                &command,
                accept
            )
            .is_err());
        // Its proof from an earlier session does not carry to a new one.
        let (reconnected, _) = h.session(GRANT_BRIDGE, "mac", "s2");
        assert!(h
            .decide(&reconnected, Some(&device), "task-a2", &command, accept)
            .is_err());
        // Content other than what was shown, or another Host.
        let mut altered = command.clone();
        altered.content_digest = "0".repeat(64);
        assert!(h
            .decide(&binding, Some(&device), "task-a2", &altered, accept)
            .is_err());
        let mut request = NativeAgentDecisionV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            decision_id: "d1".into(),
            task_id: "task-a2".into(),
            target_host_ref: "host:someone-else".into(),
            review_ref: command.review_ref.clone(),
            content_digest: command.content_digest.clone(),
            decision: accept,
        };
        assert!(h
            .service
            .submit_native_decision(&binding, Some(&device), &request, crate::storage::now_ts())
            .is_err());
        // An expired request.
        assert!(h
            .decide_at(
                &binding,
                Some(&device),
                "task-a2",
                &command,
                accept,
                command.expires_at
            )
            .is_err());
        thread::sleep(Duration::from_millis(100));
        assert!(answers(&workspace).is_empty());

        // After a reconnect, the same device proven in the new session decides.
        let (reconnected, reproven) = h.proven(GRANT_BRIDGE, "mac", "s2");
        request.target_host_ref = h.windows_host.as_str().into();
        h.service
            .submit_native_decision(
                &reconnected,
                Some(&reproven),
                &request,
                crate::storage::now_ts(),
            )
            .unwrap();
        wait_for_review_state(
            &h.service,
            "task-a2",
            &command.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        assert_eq!(answers(&workspace).len(), 1);
        h.service
            .cancel_bridge_task(GRANT_BRIDGE, "task-a2")
            .unwrap();
        wait_for_workspace_release(&h.service, &workspace);
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn stopping_or_burning_a_task_ends_every_outstanding_review() {
        for burn in [false, true] {
            let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
            let workspace = h.workspace.clone();
            h.invoke(&binding, Some(&device), "task-a3", &workspace, "work")
                .unwrap();
            let reviews = wait_for_reviews(&h.service, "task-a3", 2);
            if burn {
                h.service.purge_bridge_authority(GRANT_BRIDGE).unwrap();
            } else {
                h.service
                    .cancel_bridge_task(GRANT_BRIDGE, "task-a3")
                    .unwrap();
            }
            let stopped = h.service.task_status("task-a3");
            if let Ok(stopped) = stopped {
                assert_eq!(stopped.state, NativeAgentTaskStateV1::Cancelled);
                assert!(stopped.native_decisions.is_empty());
            }
            for review in &reviews {
                for decision in [NativeDecisionV1::Accept, NativeDecisionV1::Decline] {
                    assert!(h
                        .decide(&binding, Some(&device), "task-a3", review, decision)
                        .is_err());
                }
            }
            wait_for_workspace_release(&h.service, &workspace);
            assert!(answers(&workspace).is_empty(), "burn={burn}");
            let _ = fs::remove_dir_all(script_root);
        }
    }

    #[test]
    fn a_request_codex_resolved_or_changed_first_is_not_answered() {
        let resolved_first = r#"    *'"id":3,'*)
      {ACK}
      printf '%s\n' '{"id":"approval-cmd","method":"item/commandExecution/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"cmd-1","startedAtMs":1,"command":"cargo test","cwd":"/repo"}}'
      (
        while [ ! -f "$PWD/resolve" ]; do fixture_alive || exit 0; sleep 0.01; done
        printf '%s\n' '{"method":"serverRequest/resolved","params":{"threadId":"native-thread","requestId":"approval-cmd"}}'
        while [ ! -f "$PWD/finish-turn" ]; do fixture_alive || exit 0; sleep 0.01; done
        {COMPLETED}
      ) &
      ;;
    *'"id":"approval-cmd"'*) printf '%s\n' "$line" >> "$PWD/answers" ;;
"#;
        let (mut h, script_root, binding, device) = approval_fixture(resolved_first);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a4", &workspace, "work")
            .unwrap();
        let command = wait_for_reviews(&h.service, "task-a4", 1).remove(0);
        fs::write(workspace.join("resolve"), b"").unwrap();
        wait_for_review_state(
            &h.service,
            "task-a4",
            &command.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a4",
                &command,
                NativeDecisionV1::Accept
            )
            .is_err());
        fs::write(workspace.join("finish-turn"), b"").unwrap();
        assert_eq!(
            wait_for_terminal(&h.service, "task-a4").state,
            NativeAgentTaskStateV1::Completed
        );
        assert!(answers(&workspace).is_empty());
        let _ = fs::remove_dir_all(script_root);

        // A diff that changes after it was shown can no longer be approved.
        let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a5", &workspace, "work")
            .unwrap();
        let patch = review_of_kind(
            &wait_for_reviews(&h.service, "task-a5", 2),
            NativeDecisionKindV1::FileChange,
        );
        fs::write(workspace.join("patch-updated"), b"").unwrap();
        wait_for_file(&workspace.join("patch-update-sent"));
        let mut changed = patch.clone();
        for _ in 0..500 {
            changed = review_of_kind(
                &h.service.task_status("task-a5").unwrap().native_decisions,
                NativeDecisionKindV1::FileChange,
            );
            if !changed.can_accept {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            changed.unavailable_reason.as_deref(),
            Some("native_decision_content_changed")
        );
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a5",
                &patch,
                NativeDecisionV1::Accept
            )
            .is_err());
        h.decide(
            &binding,
            Some(&device),
            "task-a5",
            &changed,
            NativeDecisionV1::Decline,
        )
        .unwrap();
        wait_for_review_state(
            &h.service,
            "task-a5",
            &patch.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        assert_eq!(
            answers(&workspace),
            [r#"{"id":7,"result":{"decision":"decline"}}"#]
        );
        h.service
            .cancel_bridge_task(GRANT_BRIDGE, "task-a5")
            .unwrap();
        wait_for_workspace_release(&h.service, &workspace);
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn unsupported_requests_stay_visible_unanswerable_and_bounded() {
        let many = r#"    *'"id":3,'*)
      {ACK}
      printf '%s\n' '{"id":"perm-1","method":"item/permissions/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"p","permissions":{"network":{"enabled":true}}}}'
      printf '%s\n' '{"id":"ask-1","method":"item/tool/requestUserInput","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"q","questions":[]}}'
      printf '%s\n' '{"id":"cmd-x","method":"item/commandExecution/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"c","command":{"injected":true},"cwd":"/"}}'
      printf '%s\n' '{"id":"cmd-4","method":"item/commandExecution/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"c4","command":"ls","cwd":"/"}}'
      printf '%s\n' '{"id":"cmd-other","method":"item/commandExecution/requestApproval","params":{"threadId":"other-thread","turnId":"other-turn","itemId":"c5","command":"ls","cwd":"/"}}'
      touch "$PWD/all-sent"
      ;;
    *'"result"'*) printf '%s\n' "$line" >> "$PWD/answers" ;;
    *'turn/interrupt'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"interrupted","error":null}}}'
      ;;
"#;
        let (mut h, script_root, binding, device) = approval_fixture(many);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a6", &workspace, "work")
            .unwrap();
        wait_for_file(&workspace.join("all-sent"));
        thread::sleep(Duration::from_millis(200));
        let reviews = h.service.task_status("task-a6").unwrap().native_decisions;
        // Bounded: the first three are shown; the rest still wait, visibly.
        assert_eq!(reviews.len(), MAX_NATIVE_DECISION_REVIEWS);
        for review in &reviews {
            assert_eq!(review.kind, NativeDecisionKindV1::Unsupported);
            assert!(!review.can_accept && !review.can_decline);
            assert_eq!(review.content, None);
            for decision in [NativeDecisionV1::Accept, NativeDecisionV1::Decline] {
                assert!(h
                    .decide(&binding, Some(&device), "task-a6", review, decision)
                    .is_err());
            }
        }
        assert_eq!(
            h.service.task_status("task-a6").unwrap().code.as_deref(),
            Some(AWAITING_NATIVE_DECISION_CODE)
        );
        assert!(answers(&workspace).is_empty());
        h.service
            .cancel_bridge_task(GRANT_BRIDGE, "task-a6")
            .unwrap();
        wait_for_workspace_release(&h.service, &workspace);
        assert!(answers(&workspace).is_empty());
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn app_server_loss_during_review_ends_it_without_an_answer() {
        let dies = r#"    *'"id":3,'*)
      {ACK}
      printf '%s\n' '{"id":"approval-cmd","method":"item/commandExecution/requestApproval","params":{"threadId":"native-thread","turnId":"native-turn","itemId":"cmd-1","command":"cargo test","cwd":"/repo"}}'
      while [ ! -f "$PWD/die" ]; do fixture_alive || exit 0; sleep 0.01; done
      exit 0
      ;;
"#;
        let (mut h, script_root, binding, device) = approval_fixture(dies);
        let workspace = h.workspace.clone();
        h.invoke(&binding, Some(&device), "task-a7", &workspace, "work")
            .unwrap();
        let command = wait_for_reviews(&h.service, "task-a7", 1).remove(0);
        fs::write(workspace.join("die"), b"").unwrap();
        let terminal = wait_for_terminal(&h.service, "task-a7");
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            terminal.code.as_deref(),
            Some("native_agent_outcome_unknown")
        );
        assert!(terminal.native_decisions.is_empty());
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a7",
                &command,
                NativeDecisionV1::Accept
            )
            .is_err());
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn a_task_whose_requester_proved_no_device_takes_no_remote_decisions() {
        let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
        let workspace = h.workspace.clone();
        // Held for Review (no device, so no grant), then accepted here.
        let held = h
            .invoke(&binding, None, "task-a8", &workspace, "work")
            .unwrap();
        assert_eq!(held.code.as_deref(), Some(admission::REVIEW_REQUIRED_CODE));
        h.service
            .accept_invocation_review("task-a8", Some(&binding), crate::storage::now_ts())
            .unwrap();
        let reviews = wait_for_reviews(&h.service, "task-a8", 2);
        // Nobody could give an answer, so none is offered, with the reason.
        for review in &reviews {
            assert!(!review.can_accept && !review.can_decline);
            assert_eq!(
                review.unavailable_reason.as_deref(),
                Some("native_decision_requester_unverified")
            );
        }
        let command = review_of_kind(&reviews, NativeDecisionKindV1::CommandExecution);
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a8",
                &command,
                NativeDecisionV1::Accept
            )
            .is_err());
        // A later proof cannot attach itself through a replayed invocation.
        let _ = h.invoke(&binding, Some(&device), "task-a8", &workspace, "work");
        assert!(h
            .decide(
                &binding,
                Some(&device),
                "task-a8",
                &command,
                NativeDecisionV1::Accept
            )
            .is_err());
        thread::sleep(Duration::from_millis(100));
        assert!(answers(&workspace).is_empty());
        h.service
            .cancel_bridge_task(GRANT_BRIDGE, "task-a8")
            .unwrap();
        wait_for_workspace_release(&h.service, &workspace);
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn a_reviewed_task_with_a_proven_requester_takes_its_decisions() {
        let (mut h, script_root, binding, device) = approval_fixture(APPROVAL_TURN);
        // No grant applies to this other folder, so it waits for Review.
        let workspace = h.workspace.parent().unwrap().join("Other");
        fs::create_dir_all(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let held = h
            .invoke(&binding, Some(&device), "task-a9", &workspace, "work")
            .unwrap();
        assert_eq!(held.code.as_deref(), Some(admission::REVIEW_REQUIRED_CODE));
        h.service
            .accept_invocation_review("task-a9", Some(&binding), crate::storage::now_ts())
            .unwrap();
        let command = review_of_kind(
            &wait_for_reviews(&h.service, "task-a9", 2),
            NativeDecisionKindV1::CommandExecution,
        );
        h.decide(
            &binding,
            Some(&device),
            "task-a9",
            &command,
            NativeDecisionV1::Decline,
        )
        .unwrap();
        wait_for_review_state(
            &h.service,
            "task-a9",
            &command.review_ref,
            NativeDecisionStateV1::Resolved,
        );
        assert_eq!(
            answers(&workspace),
            [r#"{"id":"approval-cmd","result":{"decision":"decline"}}"#]
        );
        h.service
            .cancel_bridge_task(GRANT_BRIDGE, "task-a9")
            .unwrap();
        wait_for_workspace_release(&h.service, &workspace);
        let _ = fs::remove_dir_all(script_root);
    }

    #[test]
    fn statuses_carry_reviews_only_while_running_and_within_bounds() {
        let review = NativeDecisionReviewV1 {
            review_ref: "native-decision:r".into(),
            kind: NativeDecisionKindV1::CommandExecution,
            state: NativeDecisionStateV1::Pending,
            submitted: None,
            can_accept: true,
            can_decline: true,
            unavailable_reason: None,
            content: Some(NativeDecisionContentV1 {
                command: Some("ls".into()),
                cwd: Some("/".into()),
                ..NativeDecisionContentV1::default()
            }),
            content_digest: String::new(),
            requested_at: 1,
            expires_at: 2,
        };
        let digest = |review: &NativeDecisionReviewV1| {
            native_decision_content_digest(review.kind, review.content.as_ref())
        };
        let wire = |state, reviews: Vec<NativeDecisionReviewV1>| NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task".into(),
            executing_host_ref: "host:windows".into(),
            status: NativeAgentTaskStatusV1 {
                schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: "task".into(),
                agent_id: CODEX_CAPABILITY_ID.into(),
                workspace_name: "REPZ".into(),
                session_reused: false,
                state,
                result: None,
                code: None,
                output: None,
                conversation: None,
                native_decisions: reviews,
            },
        };
        let mut valid = review.clone();
        valid.content_digest = digest(&valid);
        validate_status(&wire(NativeAgentTaskStateV1::Running, vec![valid.clone()])).unwrap();
        // Not after the task ended.
        assert!(validate_status(&wire(
            NativeAgentTaskStateV1::Completed,
            vec![valid.clone()]
        ))
        .is_err());
        // Not more than the bound.
        assert!(validate_status(&wire(
            NativeAgentTaskStateV1::Running,
            vec![valid.clone(); MAX_NATIVE_DECISION_REVIEWS + 1]
        ))
        .is_err());
        // Not with a digest of other content, an approvable unsupported or
        // content-less request, or oversized content.
        let mut forged = valid.clone();
        forged.content.as_mut().unwrap().command = Some("rm -rf /".into());
        assert!(validate_status(&wire(NativeAgentTaskStateV1::Running, vec![forged])).is_err());
        let mut unsupported = valid.clone();
        unsupported.kind = NativeDecisionKindV1::Unsupported;
        unsupported.content_digest = digest(&unsupported);
        assert!(
            validate_status(&wire(NativeAgentTaskStateV1::Running, vec![unsupported])).is_err()
        );
        let mut blind = valid.clone();
        blind.content = None;
        blind.content_digest = digest(&blind);
        assert!(validate_status(&wire(NativeAgentTaskStateV1::Running, vec![blind])).is_err());
        let mut huge = valid.clone();
        huge.content.as_mut().unwrap().command =
            Some("x".repeat(MAX_NATIVE_DECISION_CONTENT_BYTES));
        huge.content_digest = digest(&huge);
        assert!(validate_status(&wire(NativeAgentTaskStateV1::Running, vec![huge])).is_err());
        let mut bad_ref = valid;
        bad_ref.review_ref = "approval-cmd".into();
        assert!(validate_status(&wire(NativeAgentTaskStateV1::Running, vec![bad_ref])).is_err());
    }

    #[test]
    fn codex_capability_distinguishes_compatible_incompatible_and_absent_interfaces() {
        let root = std::env::temp_dir().join(format!("pastey-native-compat-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let compatible = compatibility_fixture(&root, 0);
        assert_eq!(
            codex_compatibility_at(&compatible),
            NativeAgentCapabilityStateV1::Available
        );
        let incompatible = compatibility_fixture(&root, 1);
        assert_eq!(
            codex_compatibility_at(&incompatible),
            NativeAgentCapabilityStateV1::Incompatible
        );
        assert_eq!(
            codex_compatibility_at(&root.join("codex-absent")),
            NativeAgentCapabilityStateV1::Unavailable
        );
        let _ = fs::remove_dir_all(root);
    }

    /// A Bridge-owned Codex invocation whose Review was already accepted, for
    /// tests of what happens after a remote invocation started.
    fn start_accepted_bridge_codex_task(
        service: &mut NativeAgentServiceV1,
        bridge_id: &str,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        service.codex.use_executable(executable);
        let input = capability::codex::codex_input(workspace, task, true)?;
        service.with_bridge_task_owner(bridge_id, task_id, |service| {
            service.start_invocation_in_movement(
                CODEX_CAPABILITY_ID,
                task_id,
                &input,
                None,
                Some(NativeConversationOwnerV1::Peer {
                    bridge_id: bridge_id.into(),
                    peer_host_ref: TEST_PEER_HOST.into(),
                }),
            )
        })
    }

    const TEST_PEER_HOST: &str = "host:test-requester";

    /// The executor's exact current session with a movement's source peer.
    fn test_movement_binding(bridge_id: &str) -> HostSessionBinding {
        HostSessionBinding::new(
            bridge_id,
            crate::host_identity::HostRef::from_device_id("test-executor").unwrap(),
            crate::host_identity::HostRef::from_device_id("test-source").unwrap(),
            "executor-session",
            "source-session",
            "source-route",
            crate::storage::now_ts() + 3_600,
        )
        .unwrap()
    }

    /// Lands an approved outbound workspace over its prepare session and
    /// accepts the executor Review it then waits for, as this Host's user
    /// would.
    fn land_and_accept(
        service: &mut NativeAgentServiceV1,
        executable: &Path,
        movement_id: &str,
        task_workspace: &Path,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let bridge_id = service
            .workspace_movements
            .get(movement_id)
            .and_then(|record| record.bridge_id.clone())
            .expect("a Bridge-prepared movement");
        let binding = test_movement_binding(&bridge_id);
        service
            .movement_bindings
            .insert(movement_id.into(), binding.clone());
        let held = service.start_received_workspace_task_with_executable(
            executable,
            movement_id,
            task_workspace,
        )?;
        if service.invocation_review_route(&held.task_id).is_none() {
            return Ok(held);
        }
        service.accept_invocation_review(&held.task_id, Some(&binding), crate::storage::now_ts())
    }

    fn wait_for_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
        for _ in 0..300 {
            let status = service.task_status(task_id).unwrap();
            if status.state != NativeAgentTaskStateV1::Running {
                return status;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture native Agent did not complete")
    }

    fn wait_for_workspace_release(service: &NativeAgentServiceV1, workspace: &Path) {
        // Occupancy is keyed by the canonical workspace.
        let workspace = &workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        for _ in 0..300 {
            if service.workspace_occupant(workspace).is_none() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture native Agent did not release its workspace")
    }

    /// Live runs use the user's own model settings, which may reason for
    /// minutes; each change of state or pending request is printed.
    fn wait_for_live_terminal(
        service: &NativeAgentServiceV1,
        task_id: &str,
    ) -> NativeAgentTaskStatusV1 {
        let started = std::time::Instant::now();
        let mut last = String::new();
        while started.elapsed() < Duration::from_secs(600) {
            let status = service.task_status(task_id).unwrap();
            let summary = format!(
                "state={:?} code={:?} decisions={:?}",
                status.state,
                status.code,
                status
                    .native_decisions
                    .iter()
                    .map(|review| (&review.kind, &review.state))
                    .collect::<Vec<_>>()
            );
            if summary != last {
                eprintln!("[live {:>4}s] {summary}", started.elapsed().as_secs());
                last = summary;
            }
            if status.state != NativeAgentTaskStateV1::Running {
                return status;
            }
            thread::sleep(Duration::from_millis(250));
        }
        panic!("installed Codex did not return a bounded terminal outcome")
    }

    #[test]
    fn cancelling_without_an_adapter_session_still_records_cancellation_durably() {
        let root = std::env::temp_dir().join(format!("pastey-native-cancel-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = durable_paths(&root);
        let task_id = "native-agent:uncertain-after-restart";
        {
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            service.tasks.lock().unwrap().insert(
                task_id.into(),
                NativeAgentTaskStatusV1 {
                    schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                    task_id: task_id.into(),
                    agent_id: CODEX_CAPABILITY_ID.into(),
                    workspace_name: "workspace".into(),
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Interrupted,
                    result: None,
                    code: Some("native_agent_outcome_unknown".into()),
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                },
            );
            service.task_workspaces.insert(task_id.into(), root.clone());
            service
                .task_bridges
                .insert(task_id.into(), "room-uncertain".into());
            service
                .persist_envelope(task_id, Some(&NativeAgentServiceV1::task_digest("task")))
                .unwrap();
        }
        // After restart the Codex adapter has no session for this uncertain
        // task. Cancellation is still recorded; only the native stop is
        // uncertain.
        let mut restarted = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert!(!restarted.codex.has_task(task_id));
        let cancelled = restarted.cancel_task(task_id).unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            cancelled.code.as_deref(),
            Some("native_agent_cancel_delivery_uncertain")
        );
        let stored = crate::storage::get_native_agent_envelope(&paths, task_id)
            .unwrap()
            .unwrap();
        let persisted: PersistedNativeAgentEnvelopeV1 =
            serde_json::from_str(&stored.record_json).unwrap();
        assert_eq!(persisted.task, cancelled);
        let again = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(again.task_status(task_id).unwrap(), cancelled);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancelling_an_old_task_never_touches_a_replacement_session() {
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let old = service
            .start_codex_task_with_executable(&agent, &workspace, "first")
            .unwrap();
        wait_for_terminal(&service, &old.task_id);
        let canonical = workspace.canonicalize().unwrap();
        let old_controller = service.codex.session_controller(&canonical).unwrap();
        // Movement cleanup closes the session; a later task starts a new one.
        service.codex.shutdown_session(&canonical);
        let newer = service
            .start_codex_task_with_executable(&agent, &workspace, "second")
            .unwrap();
        assert!(!newer.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &newer.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let replacement = service.codex.session_controller(&canonical).unwrap();
        assert!(!Arc::ptr_eq(&old_controller, &replacement));
        // The old task's stop acts only on its own app-server.
        let _ = service.codex.cancel(&old.task_id);
        assert!(Arc::ptr_eq(
            &service.codex.session_controller(&canonical).unwrap(),
            &replacement
        ));
        // Once that app-server is gone the stop is uncertain, and the
        // replacement is still untouched.
        drop(old_controller);
        assert!(service.codex.cancel(&old.task_id).is_err());
        service.codex.release(&old.task_id);
        assert!(Arc::ptr_eq(
            &service.codex.session_controller(&canonical).unwrap(),
            &replacement
        ));
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn releasing_one_task_leaves_other_tasks_adapters_and_sessions_alone() {
        let (root, workspace, agent) = fixture();
        let other_workspace = root.join("other");
        fs::create_dir(&other_workspace).unwrap();
        let codex = Arc::new(CodexNativeAdapterV1::default());
        let fake = Arc::new(capability::fake_longjob::FakeLongJobAdapterV1::default());
        let mut service =
            NativeAgentServiceV1::with_adapters(codex.clone(), vec![codex.clone(), fake.clone()]);
        let first = service
            .start_codex_task_with_executable(&agent, &workspace, "first")
            .unwrap();
        let other = service
            .start_codex_task_with_executable(&agent, &other_workspace, "other")
            .unwrap();
        wait_for_terminal(&service, &first.task_id);
        wait_for_terminal(&service, &other.task_id);
        fake.hold();
        let job_input: OpaqueCapabilityPayloadV1 =
            serde_json::from_value(json!({ "steps": 1, "payload": 1 })).unwrap();
        service
            .start_invocation_in_movement(
                capability::fake_longjob::FAKE_LONGJOB_ID,
                "task-fake-job",
                &job_input,
                None,
                None,
            )
            .unwrap();
        let canonical = workspace.canonicalize().unwrap();
        let other_canonical = other_workspace.canonicalize().unwrap();
        // A later session in the first workspace is not the first task's.
        let first_controller = codex.session_controller(&canonical).unwrap();
        codex.shutdown_session(&canonical);
        let later = service
            .start_codex_task_with_executable(&agent, &workspace, "later")
            .unwrap();
        wait_for_terminal(&service, &later.task_id);
        let later_controller = codex.session_controller(&canonical).unwrap();
        assert!(!Arc::ptr_eq(&first_controller, &later_controller));

        codex.release(&first.task_id);
        assert!(!codex.has_task(&first.task_id));
        assert!(codex.has_task(&other.task_id));
        assert!(codex.has_task(&later.task_id));
        assert!(Arc::ptr_eq(
            &codex.session_controller(&canonical).unwrap(),
            &later_controller
        ));
        assert!(codex.has_session(&other_canonical));
        assert!(!fake.cancel_requested("task-fake-job"));
        // Releasing another adapter's task id is a no-op on each side.
        codex.release("task-fake-job");
        NativeCapabilityAdapterV1::release(fake.as_ref(), &other.task_id);
        assert!(!fake.cancel_requested("task-fake-job"));
        assert!(codex.has_task(&other.task_id));
        assert!(codex.has_session(&other_canonical));
        // Releasing the other task ends only its own session.
        codex.release(&other.task_id);
        assert!(!codex.has_session(&other_canonical));
        assert!(Arc::ptr_eq(
            &codex.session_controller(&canonical).unwrap(),
            &later_controller
        ));
        fake.release();
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shutdown_reaches_each_registered_adapter_exactly_once() {
        // The typed Codex handle is the registered Codex adapter, not a
        // second owner of it.
        let mut service = NativeAgentServiceV1::default();
        assert_eq!(service.adapters.len(), 1);
        assert_eq!(
            Arc::as_ptr(&service.adapters[0]) as *const (),
            Arc::as_ptr(&service.codex) as *const ()
        );
        service.shutdown();
        assert_eq!(service.codex.shutdown_count(), 1);

        let codex = Arc::new(CodexNativeAdapterV1::default());
        let fake = Arc::new(capability::fake_longjob::FakeLongJobAdapterV1::default());
        let mut both =
            NativeAgentServiceV1::with_adapters(codex.clone(), vec![codex.clone(), fake.clone()]);
        both.shutdown();
        assert_eq!(codex.shutdown_count(), 1);
        assert_eq!(fake.shutdowns(), 1);
    }

    #[test]
    fn original_workspace_tasks_resume_one_host_private_codex_session() {
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task_with_executable(&agent, &workspace, "create a note")
            .unwrap();
        assert!(!first.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &first.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let second = service
            .start_codex_task_with_executable(&agent, &workspace, "refine the note")
            .unwrap();
        assert!(second.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &second.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let unrelated = root.join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        let third = service
            .start_codex_task_with_executable(&agent, &unrelated, "new project")
            .unwrap();
        assert!(!third.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &third.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "requires an installed, authenticated native Codex app-server"]
    fn installed_codex_modifies_a_disposable_original_workspace_and_reuses_its_session() {
        if codex_compatibility_at(Path::new("codex")) != NativeAgentCapabilityStateV1::Available {
            panic!("installed Codex app-server is unavailable");
        }
        let root =
            std::env::temp_dir().join(format!("pastey-native-agent-live-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let file = workspace.join("pastey-native-agent-acceptance.txt");
        // A local task's native approvals are never answerable from Pastey,
        // so this run asks for none and may write only its disposable
        // workspace.
        #[cfg(unix)]
        let codex = installed_codex_with(
            &root,
            &[
                "approval_policy=\"never\"",
                "sandbox_mode=\"workspace-write\"",
            ],
        );
        #[cfg(not(unix))]
        let codex = capability::codex::installed_codex_program()
            .expect("an installed Codex")
            .0;
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task_with_executable(
                &codex,
                &workspace,
                "Create pastey-native-agent-acceptance.txt in this workspace containing exactly `first native turn`. Do not only describe the change; make it.",
            )
            .unwrap();
        assert!(!first.session_reused);
        assert_eq!(
            wait_for_live_terminal(&service, &first.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap().trim(),
            "first native turn"
        );
        let second = service
            .start_codex_task_with_executable(
                &codex,
                &workspace,
                "In the file you created in the preceding turn, append exactly one new line: `second native turn`. Make the edit now.",
            )
            .unwrap();
        assert!(second.session_reused);
        assert_eq!(
            wait_for_live_terminal(&service, &second.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        // Whether the model ends the file with a newline is its choice.
        assert_eq!(
            fs::read_to_string(&file)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["first native turn", "second native turn"]
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    /// D1 live: what this Host's real installations offer. On a Mac with
    /// only the desktop app, the bundled Codex is found and named.
    #[test]
    #[ignore = "observes this Host's real Codex installations"]
    fn installed_codex_is_discovered_with_its_backend() {
        let report = capability::codex::codex_probe_at(Path::new("codex"), Duration::from_secs(30));
        eprintln!(
            "installed Codex: state={:?} backend={:?} reason={:?} detail={:?}",
            report.state, report.backend, report.reason, report.detail
        );
        assert_eq!(report.state, NativeCapabilityStateForTest::Available);
    }

    use NativeAgentCapabilityStateV1 as NativeCapabilityStateForTest;

    /// A live launcher for the installed Codex with configuration values
    /// overridden for this run through Codex's own `-c` flag; the user's
    /// configuration is not touched.
    #[cfg(unix)]
    fn installed_codex_with(root: &Path, overrides: &[&str]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let (program, backend) =
            capability::codex::installed_codex_program().expect("an installed Codex");
        eprintln!("live run uses the {backend} backend: {}", program.display());
        let overrides: String = overrides
            .iter()
            .map(|value| format!(" -c '{value}'"))
            .collect();
        let wrapper = root.join("codex-live");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec '{}'{overrides} \"$@\"\n",
                program.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        wrapper
    }

    /// D1 live, product path: a Bridge task from a verified requesting
    /// device runs on the installed Codex (on a Mac with only the desktop
    /// app, its bundled Codex) under the user's own approval policy. Each
    /// native approval is shown to and decided by that requester through
    /// `submit_native_decision`, and the result comes back as the task's.
    #[test]
    #[ignore = "requires an installed, authenticated Codex app-server"]
    fn installed_codex_runs_a_bridge_task_with_its_requesters_approval() {
        let mut h = unattended_fixture();
        // A read-only sandbox that may ask: the write cannot happen without
        // the requester's native approval.
        #[cfg(unix)]
        let codex = installed_codex_with(
            &h.root,
            &[
                "approval_policy=\"on-request\"",
                "sandbox_mode=\"read-only\"",
            ],
        );
        #[cfg(not(unix))]
        let codex = capability::codex::installed_codex_program()
            .expect("an installed Codex")
            .0;
        h.service.codex.use_executable(&codex);
        let (binding, device) = h.proven(GRANT_BRIDGE, "mac", "s-live");
        h.trust_and_grant(&device, 24, 5);
        let workspace = h.workspace.clone();
        h.invoke(
            &binding,
            Some(&device),
            "task-live",
            &workspace,
            "Using a single shell command, create a file named pastey-live.txt in this workspace containing exactly `desktop codex`. Then reply with the word done.",
        )
        .unwrap();
        let started = std::time::Instant::now();
        let mut approved = 0;
        let terminal =
            loop {
                assert!(
                    started.elapsed() < Duration::from_secs(600),
                    "no bounded outcome"
                );
                let status = h.service.task_status("task-live").unwrap();
                if status.state != NativeAgentTaskStateV1::Running {
                    break status;
                }
                if let Some(review) = status.native_decisions.iter().find(|review| {
                    review.state == NativeDecisionStateV1::Pending && review.can_accept
                }) {
                    eprintln!(
                        "[live {:>4}s] requester approves {:?}: {:?}",
                        started.elapsed().as_secs(),
                        review.kind,
                        review.content
                    );
                    let review = review.clone();
                    h.decide(
                        &binding,
                        Some(&device),
                        "task-live",
                        &review,
                        NativeDecisionV1::Accept,
                    )
                    .unwrap();
                    approved += 1;
                }
                thread::sleep(Duration::from_millis(250));
            };
        eprintln!(
            "terminal: state={:?} code={:?} result={:?} approvals={approved}",
            terminal.state, terminal.code, terminal.result
        );
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Completed);
        #[cfg(unix)]
        assert!(approved > 0, "the run asked for no native approval");
        assert_eq!(
            fs::read_to_string(workspace.join("pastey-live.txt"))
                .unwrap()
                .trim(),
            "desktop codex"
        );
    }

    #[test]
    fn terminal_success_requires_the_exact_native_thread_turn_and_clean_completion() {
        let cases = [
            (
                r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"failed","error":{"message":"no"}}}}"#,
                NativeAgentTaskStateV1::Failed,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"interrupted","error":null}}}"#,
                NativeAgentTaskStateV1::Interrupted,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"cancelled","error":null}}}"#,
                NativeAgentTaskStateV1::Cancelled,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#,
                NativeAgentTaskStateV1::Interrupted,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"other-turn","items":[],"status":"completed","error":null}}}"#,
                NativeAgentTaskStateV1::Interrupted,
            ),
            (
                r#"{"method":"turn/completed","params":{}}"#,
                NativeAgentTaskStateV1::Interrupted,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":{"message":"no"}}}}"#,
                NativeAgentTaskStateV1::Interrupted,
            ),
        ];
        for (index, (terminal, expected)) in cases.into_iter().enumerate() {
            let (root, workspace, agent) = fixture_with_terminal(terminal);
            let mut service = NativeAgentServiceV1::default();
            let started = service
                .start_codex_task_with_executable(&agent, &workspace, "run")
                .unwrap();
            let terminal = wait_for_terminal(&service, &started.task_id);
            assert_eq!(terminal.state, expected, "case {index}");
            assert_ne!(terminal.state, NativeAgentTaskStateV1::Completed);
            service.shutdown();
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn cancellation_wins_a_late_native_completion() {
        let (root, workspace, agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "wait")
            .unwrap();
        let cancelled = service.cancel_task(&started.task_id).unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delayed_native_terminal_remains_running_without_an_execution_deadline() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, workspace, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; sleep 1; echo '{terminal}'"
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "long task")
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Running
        );
        assert_eq!(
            wait_for_terminal(&service, &started.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn uncertain_turn_start_ack_is_interrupted_without_a_second_turn() {
        let (root, workspace, agent) = fixture_with_turn_start_behavior("");
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "start once")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            terminal.code.as_deref(),
            Some("native_agent_outcome_unknown")
        );
        assert!(service
            .workspace_has_authoritative_owner(&workspace, None, None)
            .unwrap());
        assert!(service
            .start_codex_task_with_executable_and_id(
                &agent,
                &started.task_id,
                &workspace,
                "start once",
            )
            .is_ok());
        assert!(service
            .start_codex_task_with_executable(&agent, &workspace, "must not overlap")
            .is_err());
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_cancel_terminates_an_uncertain_turn_start_and_releases_its_workspace() {
        let (root, workspace, agent) = fixture_with_turn_start_behavior("");
        let (fresh_root, _, fresh_agent) = fixture();
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "start once")
            .unwrap();
        let unknown = wait_for_terminal(&service, &started.task_id);
        assert_eq!(unknown.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            unknown.code.as_deref(),
            Some("native_agent_outcome_unknown")
        );
        assert!(service
            .workspace_occupant(&workspace.canonicalize().unwrap())
            .is_some());

        assert_eq!(
            service.cancel_task(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        wait_for_workspace_release(&service, &workspace);
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        let fresh = service
            .start_codex_task_with_executable(&fresh_agent, &workspace, "start fresh")
            .unwrap();
        assert!(!fresh.session_reused);
        assert_eq!(
            wait_for_terminal(&service, &fresh.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(fresh_root);
    }

    #[test]
    fn native_observation_loss_is_interrupted_unknown() {
        let (root, workspace, agent) = fixture_with_turn_start_behavior(
            "echo '{\"id\":3,\"result\":{\"turn\":{\"id\":\"native-turn\"}}}'; exit 0",
        );
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "observe")
            .unwrap();
        let terminal = wait_for_terminal(&service, &started.task_id);
        assert_eq!(terminal.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            terminal.code.as_deref(),
            Some("native_agent_outcome_unknown")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_holds_workspace_until_native_observer_exits() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, workspace, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; sleep 1; echo '{terminal}'"
        ));
        let mut service = NativeAgentServiceV1::default();
        let started = service
            .start_codex_task_with_executable(&agent, &workspace, "wait")
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            service.cancel_task(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert!(service
            .start_codex_task_with_executable(&agent, &workspace, "must wait")
            .is_err());
        thread::sleep(Duration::from_millis(1100));
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert!(service
            .start_codex_task_with_executable(&agent, &workspace, "may start now")
            .is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancelling_workspace_task_cancels_movement_without_releasing_native_occupancy() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, workspace, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; sleep 1; echo '{terminal}'"
        ));
        let mut service = NativeAgentServiceV1::default();
        service
            .accept_bridge_workspace_prepare_from(
                "room-cancel-host",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-cancel-host".into(),
                    task_id: "task-cancel-host".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:target".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "work".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 13,
                },
            )
            .unwrap();
        let started =
            land_and_accept(&mut service, &agent, "movement-cancel-host", &workspace).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            service.cancel_task(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert_eq!(
            service
                .movement_status("movement-cancel-host")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        assert!(service
            .start_codex_task_with_executable(&agent, &workspace, "must not overlap")
            .is_err());
        thread::sleep(Duration::from_millis(1100));
        assert_eq!(
            service.task_status(&started.task_id).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert_eq!(
            service
                .movement_status("movement-cancel-host")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        wait_for_workspace_release(&service, &workspace);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn workspace_monitor_keeps_agent_running_without_a_status_timeout() {
        let (root, _, _) = fixture();
        let paths = durable_paths(&root);
        let runtime = Arc::new(
            crate::host_runtime::HostRuntime::new(
                paths,
                test_config(),
                Arc::new(NoopEventSink),
                Arc::new(NoopTaskSpawner),
            )
            .unwrap(),
        );
        {
            let mut service = runtime.native_agents.lock();
            service.tasks.lock().unwrap().insert(
                "task-long-movement".into(),
                NativeAgentTaskStatusV1 {
                    schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                    task_id: "task-long-movement".into(),
                    agent_id: CODEX_CAPABILITY_ID.into(),
                    workspace_name: "workspace".into(),
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Running,
                    result: None,
                    code: None,
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                },
            );
            service.workspace_movements.insert(
                "movement-long-running".into(),
                WorkspaceMovementRecordV1 {
                    status: NativeAgentWorkspaceMovementV1 {
                        schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                        movement_id: "movement-long-running".into(),
                        task_id: "task-long-movement".into(),
                        agent_id: CODEX_CAPABILITY_ID.into(),
                        source_workspace_name: "transferred workspace".into(),
                        target_host_ref: runtime.local_host_ref.as_str().into(),
                        review_summary: "running".into(),
                        state: NativeAgentWorkspaceMovementStateV1::AgentRunning,
                        code: None,
                    },
                    source: None,
                    prepared_remote: Some(PreparedRemoteWorkspaceV1 {
                        source_host_ref: "host:source".into(),
                        task: "edit".into(),
                        resume: true,
                        source_object: movement_object(),
                        source_digest: "a".repeat(64),
                        source_bytes: 1,
                    }),
                    task_workspace: None,
                    result_snapshot: None,
                    result_identity: None,
                    expected_return_digest: None,
                    bridge_id: None,
                    apply_completed: false,
                    apply_transaction: None,
                },
            );
        }
        let watcher = tokio::spawn(monitor_received_workspace_task(
            runtime.clone(),
            "room-unavailable".into(),
            "peer-unavailable".into(),
            "movement-long-running".into(),
        ));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            runtime
                .native_agents
                .lock()
                .movement_status("movement-long-running")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::AgentRunning
        );
        runtime
            .native_agents
            .lock()
            .tasks
            .lock()
            .unwrap()
            .get_mut("task-long-movement")
            .unwrap()
            .state = NativeAgentTaskStateV1::Cancelled;
        tokio::time::timeout(Duration::from_secs(2), watcher)
            .await
            .expect("terminal task state ends the monitor")
            .unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_status_requires_the_exact_selected_host_and_cancellation_wins() {
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("request-1", "host:remote", "/remote/workspace", "task")
            .unwrap();
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: "request-1".into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: "workspace".into(),
            session_reused: true,
            state: NativeAgentTaskStateV1::Completed,
            result: Some("done".into()),
            code: None,
            output: None,
            conversation: None,
            native_decisions: Vec::new(),
        };
        let wrong = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request-1".into(),
            executing_host_ref: "host:other".into(),
            status: status.clone(),
        };
        assert!(service.record_remote_status(wrong).is_err());
        let cancelled = service
            .cancel_remote_task("request-1", "host:remote")
            .unwrap();
        assert_eq!(cancelled.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            cancelled.code.as_deref(),
            Some("native_agent_cancel_requested")
        );
        assert_eq!(
            service
                .mark_remote_cancel_delivery_uncertain("request-1")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_cancel_delivery_uncertain")
        );
        let late = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request-1".into(),
            executing_host_ref: "host:remote".into(),
            status,
        };
        assert_eq!(
            service.record_remote_status(late).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        service
            .record_remote_reconciliation(NativeAgentReconciliationV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "request-1".into(),
                movement_id: None,
                executing_host_ref: "host:remote".into(),
                task_state: NativeAgentTaskStateV1::Completed,
                movement_state: None,
                result_digest: None,
                apply_completed: false,
                code: None,
                task_output: None,
            })
            .unwrap();
        assert_eq!(
            service.task_status("request-1").unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
    }

    #[test]
    fn requester_cancellation_of_unknown_outcome_keeps_its_movement_cancelled() {
        let (root, source, _) = fixture();
        fs::write(source.join("approved.txt"), b"approved baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("native.txt"), b"native result").unwrap();
        let mut service = NativeAgentServiceV1::default();
        service
            .propose_workspace_movement(
                "movement-cancel-requester",
                "task-cancel-requester",
                &source,
                "host:remote",
                movement_object(),
                "work",
                true,
            )
            .unwrap();
        {
            let mut tasks = service.tasks.lock().unwrap();
            let task = tasks.get_mut("task-cancel-requester").unwrap();
            task.state = NativeAgentTaskStateV1::Interrupted;
            task.code = Some("native_agent_outcome_unknown".into());
        }
        assert_eq!(
            service
                .cancel_remote_task("task-cancel-requester", "host:remote")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert_eq!(
            service
                .mark_remote_cancel_delivery_uncertain("task-cancel-requester")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_cancel_delivery_uncertain")
        );
        assert_eq!(
            service
                .movement_status("movement-cancel-requester")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );

        let completed = NativeAgentTaskStatusV1 {
            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
            task_id: "task-cancel-requester".into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: "workspace".into(),
            session_reused: true,
            state: NativeAgentTaskStateV1::Completed,
            result: Some("late result".into()),
            code: None,
            output: None,
            conversation: None,
            native_decisions: Vec::new(),
        };
        assert_eq!(
            service
                .record_remote_status(NativeAgentStatusV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-cancel-requester".into(),
                    executing_host_ref: "host:remote".into(),
                    status: completed,
                })
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Cancelled
        );
        service
            .record_remote_reconciliation(NativeAgentReconciliationV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "task-cancel-requester".into(),
                movement_id: Some("movement-cancel-requester".into()),
                executing_host_ref: "host:remote".into(),
                task_state: NativeAgentTaskStateV1::Completed,
                movement_state: Some(NativeAgentWorkspaceMovementStateV1::ReturningResult),
                result_digest: Some("c".repeat(64)),
                apply_completed: false,
                code: None,
                task_output: None,
            })
            .unwrap();
        assert_eq!(
            service
                .movement_status("movement-cancel-requester")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );

        let return_metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-cancel-requester".into(),
            task_id: "task-cancel-requester".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room".into(),
            source_host_ref: "host:remote".into(),
            destination_host_ref: "host:source".into(),
            object: movement_object(),
            content_digest: "c".repeat(64),
            logical_byte_count: 0,
        };
        assert!(service
            .validate_workspace_transfer(&return_metadata, "host:source")
            .is_err());
        assert!(service
            .apply_received_workspace_return("movement-cancel-requester", &returned)
            .is_err());
        assert_eq!(
            service
                .movement_status("movement-cancel-requester")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_agent_wire_messages_reject_provider_and_session_fields() {
        let mut request = NativeAgentInvokeV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "request".into(),
            target_host_ref: "host:remote".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            workspace: "/remote/workspace".into(),
            task: "fix it".into(),
            resume: true,
        };
        assert!(validate_invoke(&request).is_ok());
        request.agent_capability = "agent.coding.other".into();
        assert!(validate_invoke(&request).is_err());
    }

    #[test]
    fn replayed_task_identity_cannot_switch_workspace() {
        let (root, workspace, agent) = fixture();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task_with_executable_and_id(&agent, "same-request", &workspace, "first")
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        assert!(service
            .start_codex_task_with_executable_and_id(&agent, "same-request", &other, "replay")
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn movement_object() -> crate::bridge_plan_v2::ManagedObjectRevisionV2 {
        crate::bridge_plan_v2::ManagedObjectRevisionV2 {
            logical_object_id: format!("managed-object:v1:{}", "a".repeat(64)),
            revision: 1,
        }
    }

    #[test]
    fn approved_workspace_movement_has_visible_review_and_exact_outbound_package() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let mut source_host = NativeAgentServiceV1::default();
        let review = source_host
            .propose_bridge_workspace_movement(
                "room-1",
                "movement-1",
                "task-1",
                &source,
                "host:remote",
                movement_object(),
                "update the project",
                true,
            )
            .unwrap();
        assert_eq!(
            review.state,
            NativeAgentWorkspaceMovementStateV1::AwaitingApproval
        );
        assert!(review.review_summary.contains("send"));
        assert!(review.review_summary.contains("return"));
        let (prepare, metadata, package) = source_host
            .approve_workspace_movement("movement-1", "room-1", "host:source", &root)
            .unwrap();
        assert_eq!(
            metadata.phase,
            NativeAgentWorkspaceTransferPhaseV1::Outbound
        );
        assert_eq!(prepare.source_digest, metadata.content_digest);
        let tree = crate::regular_file_set_transfer::materialize_package(
            &package,
            &root,
            &metadata.content_digest,
            metadata.logical_byte_count,
        )
        .unwrap();
        assert_eq!(
            fs::read(tree.join("before.txt")).unwrap(),
            b"approved baseline"
        );
        crate::regular_file_set_transfer::cleanup_package(&package);
        crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_change_after_review_blocks_outbound_packaging() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let mut service = NativeAgentServiceV1::default();
        service
            .propose_bridge_workspace_movement(
                "room-1",
                "movement-2",
                "task-2",
                &source,
                "host:remote",
                movement_object(),
                "update the project",
                true,
            )
            .unwrap();
        fs::write(source.join("before.txt"), b"changed before approval").unwrap();
        assert!(service
            .approve_workspace_movement("movement-2", "room-1", "host:source", &root)
            .is_err());
        assert_eq!(
            service.movement_status("movement-2").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::AwaitingApproval
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_movement_blocks_unrepresentable_features_before_review() {
        let (root, source, _) = fixture();
        fs::write(source.join("file.txt"), b"content").unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        let mut service = NativeAgentServiceV1::default();
        assert!(service
            .propose_workspace_movement(
                "movement-empty-dir",
                "task-empty-dir",
                &source,
                "host:remote",
                movement_object(),
                "update the project",
                true,
            )
            .is_err());
        assert!(service
            .workspace_movements
            .get("movement-empty-dir")
            .is_none());

        fs::remove_dir(source.join("empty")).unwrap();
        fs::write(source.join("NUL.txt"), b"not portable to Windows").unwrap();
        assert!(validate_workspace_transfer_fidelity(&source).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_movement_blocks_executable_modes_and_symlinks_before_review() {
        let (root, source, _) = fixture();
        let executable = source.join("run.sh");
        fs::write(&executable, b"echo native").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        assert!(validate_workspace_transfer_fidelity(&source).is_err());

        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&executable, permissions).unwrap();
        std::os::unix::fs::symlink("run.sh", source.join("link.sh")).unwrap();
        assert!(validate_workspace_transfer_fidelity(&source).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn returned_workspace_applies_once_or_stops_at_conflict_retention_pending() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("after.txt"), b"native result").unwrap();
        let mut service = NativeAgentServiceV1::default();
        service
            .propose_workspace_movement(
                "movement-3",
                "task-3",
                &source,
                "host:remote",
                movement_object(),
                "update the project",
                true,
            )
            .unwrap();
        service
            .workspace_movements
            .get_mut("movement-3")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        let applied = service
            .apply_received_workspace_return("movement-3", &returned)
            .unwrap();
        assert_eq!(
            applied.state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert_eq!(
            fs::read(source.join("after.txt")).unwrap(),
            b"native result"
        );
        assert!(!source.join("before.txt").exists());

        let conflicted = root.join("conflicted");
        fs::create_dir(&conflicted).unwrap();
        fs::write(conflicted.join("before.txt"), b"approved baseline").unwrap();
        let mut conflict_service = NativeAgentServiceV1::default();
        conflict_service
            .propose_workspace_movement(
                "movement-4",
                "task-4",
                &conflicted,
                "host:remote",
                movement_object(),
                "update the project",
                true,
            )
            .unwrap();
        fs::write(conflicted.join("local.txt"), b"local edit").unwrap();
        let conflict = conflict_service
            .apply_received_workspace_return("movement-4", &returned)
            .unwrap();
        assert_eq!(
            conflict.state,
            NativeAgentWorkspaceMovementStateV1::ApplyingResult
        );
        assert_eq!(
            conflict.code.as_deref(),
            Some("conflict_result_retention_pending")
        );
        assert_eq!(
            fs::read(conflicted.join("local.txt")).unwrap(),
            b"local edit"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conflict_result_is_retained_outside_transient_transfer_storage() {
        let (root, _, _) = fixture();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("result.txt"), b"native result").unwrap();
        let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        let retained =
            retain_conflicted_workspace(&paths, "movement-retain", "task-retain", &returned)
                .unwrap();
        let stored = crate::storage::get_native_agent_conflict(&paths, "movement-retain")
            .unwrap()
            .expect("durable conflict receipt");
        assert_eq!(stored.retained_tree, retained);
        assert!(retained.starts_with(paths.app_data_dir.join("native-agent-conflicts")));
        fs::remove_dir_all(&returned).unwrap();
        assert_eq!(
            fs::read(retained.join("result.txt")).unwrap(),
            b"native result"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn finalize_test_conflict(
        service: &mut NativeAgentServiceV1,
        paths: &crate::storage::AppPaths,
        movement_id: &str,
        returned: &Path,
    ) -> NativeAgentWorkspaceMovementV1 {
        let pending = service
            .apply_received_workspace_return(movement_id, returned)
            .unwrap();
        assert_eq!(
            pending.code.as_deref(),
            Some("conflict_result_retention_pending")
        );
        let task_id = pending.task_id;
        retain_conflicted_workspace(paths, movement_id, &task_id, returned).unwrap();
        service
            .finalize_conflict_recovery(movement_id, paths)
            .unwrap()
    }

    #[test]
    fn conflict_reveal_and_discard_validate_then_remain_idempotent() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("result.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .propose_workspace_movement(
                "movement-reveal",
                "task-reveal",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        service
            .workspace_movements
            .get_mut("movement-reveal")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        fs::write(source.join("local-change.txt"), b"local").unwrap();
        assert_eq!(
            finalize_test_conflict(&mut service, &paths, "movement-reveal", &returned).state,
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
        );
        let revealed = service
            .retained_conflict_result_for_reveal("movement-reveal")
            .unwrap();
        assert!(revealed.starts_with(fs::canonicalize(conflict_root(&paths)).unwrap()));
        assert_eq!(
            fs::read(revealed.join("result.txt")).unwrap(),
            b"native result"
        );
        assert_eq!(
            service
                .discard_retained_conflict_result("movement-reveal")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        assert!(
            crate::storage::get_native_agent_conflict(&paths, "movement-reveal")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            service
                .discard_retained_conflict_result("movement-reveal")
                .unwrap()
                .code
                .as_deref(),
            Some("conflict_result_discarded")
        );
        assert_ne!(
            service.movement_status("movement-reveal").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conflict_retention_crash_windows_never_advertise_missing_recovery() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("result.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .propose_workspace_movement(
                "movement-crash",
                "task-crash",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        before
            .workspace_movements
            .get_mut("movement-crash")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        fs::write(source.join("local-change.txt"), b"local").unwrap();
        before
            .apply_received_workspace_return("movement-crash", &returned)
            .unwrap();
        let after_missing_retention = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let interrupted = after_missing_retention
            .movement_status("movement-crash")
            .unwrap();
        assert_eq!(
            interrupted.state,
            NativeAgentWorkspaceMovementStateV1::Interrupted
        );
        assert_eq!(
            interrupted.code.as_deref(),
            Some("conflict_result_retention_required")
        );

        let mut before_terminal = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        retain_conflicted_workspace(&paths, "movement-crash", "task-crash", &returned).unwrap();
        before_terminal
            .workspace_movements
            .get_mut("movement-crash")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ApplyingResult;
        before_terminal
            .workspace_movements
            .get_mut("movement-crash")
            .unwrap()
            .status
            .code = Some("conflict_result_retention_pending".into());
        before_terminal
            .persist_envelope("task-crash", None)
            .unwrap();
        let after_retention = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert_eq!(
            after_retention
                .movement_status("movement-crash")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
        );
        assert!(after_retention
            .retained_conflict_result_for_reveal("movement-crash")
            .is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exact_conflict_return_survives_repeated_requester_restarts_and_repairs_once() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, source, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; echo run >> \"$PWD/turn-runs\"; echo '{terminal}'"
        ));
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let received = root.join("received");
        fs::create_dir(&received).unwrap();
        fs::write(received.join("baseline.txt"), b"baseline").unwrap();
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-conflict",
                "movement-conflict",
                "task-conflict",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-conflict",
                "movement-later",
                "task-later",
                &source,
                "host:remote",
                movement_object(),
                "edit later",
                true,
            )
            .unwrap();
        let (prepare, _, package) = requester
            .approve_workspace_movement(
                "movement-conflict",
                "room-conflict",
                "host:source",
                &requester_paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let mut executor = NativeAgentServiceV1::with_paths(executor_paths).unwrap();
        executor
            .accept_bridge_workspace_prepare_from(
                "room-conflict",
                &prepare.source_host_ref.clone(),
                prepare,
            )
            .unwrap();
        land_and_accept(&mut executor, &agent, "movement-conflict", &received).unwrap();
        assert_eq!(
            wait_for_terminal(&executor, "task-conflict").state,
            NativeAgentTaskStateV1::Completed
        );
        let (_, exact_snapshot, _, identity) = executor
            .captured_result_snapshot_for_return("movement-conflict")
            .unwrap();
        let first_return = root.join("first-return");
        fs::create_dir(&first_return).unwrap();
        for name in ["baseline.txt", "turn-runs"] {
            fs::copy(exact_snapshot.join(name), first_return.join(name)).unwrap();
        }
        executor.mark_result_return_pending("movement-conflict");
        let fact = executor
            .reconciliation_fact(
                "room-conflict",
                "task-conflict",
                Some("movement-conflict"),
                "host:remote",
                "host:source",
            )
            .unwrap();
        requester
            .record_bridge_remote_reconciliation("room-conflict", fact)
            .unwrap();
        fs::write(source.join("local-change.txt"), b"user edit").unwrap();
        let transfer = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-conflict".into(),
            task_id: "task-conflict".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room-conflict".into(),
            source_host_ref: "host:remote".into(),
            destination_host_ref: "host:source".into(),
            object: movement_object(),
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        };
        requester
            .validate_workspace_transfer(&transfer, "host:source")
            .unwrap();
        assert_eq!(
            requester
                .apply_received_workspace_return("movement-conflict", &first_return)
                .unwrap()
                .code
                .as_deref(),
            Some("conflict_result_retention_pending")
        );
        assert!(requester
            .workspace_movements
            .get("movement-conflict")
            .unwrap()
            .result_identity
            .as_ref()
            .is_some_and(|received| same_logical_file_set(received, &identity)));
        drop(requester);
        for _ in 0..3 {
            requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
            let pending = requester.movement_status("movement-conflict").unwrap();
            assert_eq!(
                pending.state,
                NativeAgentWorkspaceMovementStateV1::Interrupted
            );
            assert_eq!(
                pending.code.as_deref(),
                Some("conflict_result_retention_required")
            );
            assert!(movement_holds_source_ownership(&pending));
            assert!(requester
                .approve_workspace_movement(
                    "movement-later",
                    "room-conflict",
                    "host:source",
                    &requester_paths.temp_dir,
                )
                .is_err());
            drop(requester);
        }
        requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        let mut wrong = transfer.clone();
        wrong.content_digest = "0".repeat(64);
        assert!(requester
            .validate_workspace_transfer(&wrong, "host:source")
            .is_err());
        wrong = transfer.clone();
        wrong.task_id = "wrong-task".into();
        assert!(requester
            .validate_workspace_transfer(&wrong, "host:source")
            .is_err());
        wrong = transfer.clone();
        wrong.movement_id = "movement-later".into();
        assert!(requester
            .validate_workspace_transfer(&wrong, "host:source")
            .is_err());
        wrong = transfer.clone();
        wrong.source_host_ref = "host:wrong".into();
        assert!(requester
            .validate_workspace_transfer(&wrong, "host:source")
            .is_err());
        wrong = transfer.clone();
        wrong.destination_host_ref = "host:wrong".into();
        assert!(requester
            .validate_workspace_transfer(&wrong, "host:source")
            .is_err());
        requester
            .authorize_source_apply_retry("room-conflict", "movement-conflict")
            .unwrap();
        executor
            .authorize_result_return_retry(
                "room-conflict",
                &NativeAgentRetryResultReturnV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    retry_id: "retry-conflict".into(),
                    movement_id: "movement-conflict".into(),
                    task_id: "task-conflict".into(),
                    target_host_ref: "host:remote".into(),
                },
                "host:source",
                "host:remote",
            )
            .unwrap();
        let retry_return = root.join("retry-return");
        fs::create_dir(&retry_return).unwrap();
        for name in ["baseline.txt", "turn-runs"] {
            fs::copy(exact_snapshot.join(name), retry_return.join(name)).unwrap();
        }
        let retry_identity = crate::safe_file_identity::capture_regular_file_set_identity(
            &retry_return,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        let first_identity = requester
            .workspace_movements
            .get("movement-conflict")
            .unwrap()
            .result_identity
            .as_ref()
            .unwrap();
        assert_ne!(first_identity, &retry_identity);
        assert!(same_logical_file_set(first_identity, &retry_identity));
        requester
            .validate_workspace_transfer(&transfer, "host:source")
            .unwrap();
        assert_eq!(
            requester
                .apply_received_workspace_return("movement-conflict", &retry_return)
                .unwrap()
                .code
                .as_deref(),
            Some("conflict_result_retention_pending")
        );
        retain_conflicted_workspace(
            &requester_paths,
            "movement-conflict",
            "task-conflict",
            &retry_return,
        )
        .unwrap();
        assert_eq!(
            requester
                .finalize_conflict_recovery("movement-conflict", &requester_paths)
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
        );
        assert_eq!(
            fs::read(source.join("local-change.txt")).unwrap(),
            b"user edit"
        );
        assert_eq!(
            executor.task_status("task-conflict").unwrap().state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(
            fs::read_to_string(received.join("turn-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conflict_retention_repair_or_abandon_keeps_one_source_owner() {
        for failure_code in [
            "conflict_result_retention_required",
            "conflict_result_retention_failed",
        ] {
            let (root, source, _) = fixture();
            fs::write(source.join("baseline.txt"), b"baseline").unwrap();
            let returned = root.join("returned");
            fs::create_dir(&returned).unwrap();
            fs::write(returned.join("result.txt"), b"result").unwrap();
            let identity = crate::safe_file_identity::capture_regular_file_set_identity(
                &returned,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )
            .unwrap();
            let paths = durable_paths(&root);
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            service
                .propose_bridge_workspace_movement(
                    "room-retention",
                    "movement-retention",
                    "task-retention",
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
            let (_, _, package) = service
                .approve_workspace_movement(
                    "movement-retention",
                    "room-retention",
                    "host:source",
                    &paths.temp_dir,
                )
                .unwrap();
            crate::regular_file_set_transfer::cleanup_package(&package);
            service
                .tasks
                .lock()
                .unwrap()
                .get_mut("task-retention")
                .unwrap()
                .state = NativeAgentTaskStateV1::Completed;
            service
                .workspace_movements
                .get_mut("movement-retention")
                .unwrap()
                .status
                .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
            service.persist_envelope("task-retention", None).unwrap();
            fs::write(source.join("local-change.txt"), b"user edit").unwrap();
            assert_eq!(
                service
                    .apply_received_workspace_return("movement-retention", &returned)
                    .unwrap()
                    .code
                    .as_deref(),
                Some("conflict_result_retention_pending")
            );
            if failure_code == "conflict_result_retention_failed" {
                service.interrupt_workspace_movement("movement-retention", failure_code);
            }
            drop(service);
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            let pending = service.movement_status("movement-retention").unwrap();
            assert_eq!(pending.code.as_deref(), Some(failure_code));
            assert!(movement_holds_source_ownership(&pending));
            assert!(service
                .recovery_projection_for_bridge("room-retention")
                .unwrap()
                .is_some());
            service
                .authorize_source_apply_retry("room-retention", "movement-retention")
                .unwrap();
            let transfer = NativeAgentWorkspaceTransferV1 {
                schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                movement_id: "movement-retention".into(),
                task_id: "task-retention".into(),
                phase: NativeAgentWorkspaceTransferPhaseV1::Return,
                bridge_id: "room-retention".into(),
                source_host_ref: "host:remote".into(),
                destination_host_ref: "host:source".into(),
                object: movement_object(),
                content_digest: identity.digest,
                logical_byte_count: identity.byte_count,
            };
            service
                .validate_workspace_transfer(&transfer, "host:source")
                .unwrap();
            service
                .stop_bridge_task_authority("room-retention", "task-retention")
                .unwrap();
            assert_eq!(
                service.task_status("task-retention").unwrap().state,
                NativeAgentTaskStateV1::Completed
            );
            assert_eq!(
                service
                    .movement_status("movement-retention")
                    .unwrap()
                    .code
                    .as_deref(),
                Some("native_agent_recovery_abandoned")
            );
            assert!(service
                .validate_workspace_transfer(&transfer, "host:source")
                .is_err());
            assert_eq!(
                fs::read(source.join("local-change.txt")).unwrap(),
                b"user edit"
            );
            service
                .propose_bridge_workspace_movement(
                    "room-retention",
                    "movement-new",
                    "task-new",
                    &source,
                    "host:remote",
                    movement_object(),
                    "new task",
                    true,
                )
                .unwrap();
            let (_, _, package) = service
                .approve_workspace_movement(
                    "movement-new",
                    "room-retention",
                    "host:source",
                    &paths.temp_dir,
                )
                .unwrap();
            crate::regular_file_set_transfer::cleanup_package(&package);
            let _ = fs::remove_dir_all(root);
        }
    }

    fn durable_paths(root: &Path) -> crate::storage::AppPaths {
        let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        paths
    }

    fn completed_received_workspace_service(
        paths: &crate::storage::AppPaths,
        bridge_id: &str,
        movement_id: &str,
        task_id: &str,
        task_workspace: &Path,
    ) -> NativeAgentServiceV1 {
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                bridge_id,
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: movement_id.into(),
                    task_id: task_id.into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "complete remotely".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        let movement = service.workspace_movements.get_mut(movement_id).unwrap();
        movement.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
        movement.task_workspace = Some(task_workspace.to_path_buf());
        service
            .task_workspaces
            .insert(task_id.into(), task_workspace.to_path_buf());
        let mut tasks = service.tasks.lock().unwrap();
        let task = tasks.get_mut(task_id).unwrap();
        task.state = NativeAgentTaskStateV1::Completed;
        task.result = Some("Codex completed its native task.".into());
        task.code = None;
        drop(tasks);
        service.persist_envelope(task_id, None).unwrap();
        service
    }

    fn durable_conflict_fixture(
        movement_id: &str,
        task_id: &str,
    ) -> (
        PathBuf,
        crate::storage::AppPaths,
        NativeAgentServiceV1,
        crate::storage::StoredNativeAgentConflict,
    ) {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("result.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .propose_workspace_movement(
                movement_id,
                task_id,
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        service
            .workspace_movements
            .get_mut(movement_id)
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        fs::write(source.join("local-change.txt"), b"local").unwrap();
        assert_eq!(
            finalize_test_conflict(&mut service, &paths, movement_id, &returned).state,
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
        );
        let conflict = crate::storage::get_native_agent_conflict(&paths, movement_id)
            .unwrap()
            .expect("durable conflict receipt");
        (root, paths, service, conflict)
    }

    fn mark_conflict_discard_pending(service: &mut NativeAgentServiceV1, movement_id: &str) {
        let task_id = service
            .workspace_movements
            .get(movement_id)
            .unwrap()
            .status
            .task_id
            .clone();
        let movement = service.workspace_movements.get_mut(movement_id).unwrap();
        assert_eq!(
            movement.status.state,
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
        );
        movement.status.state = NativeAgentWorkspaceMovementStateV1::Cancelled;
        movement.status.code = Some("conflict_result_discard_pending".into());
        service.persist_envelope(&task_id, None).unwrap();
    }

    fn assert_discarded(
        service: &mut NativeAgentServiceV1,
        paths: &crate::storage::AppPaths,
        movement_id: &str,
    ) {
        if service
            .movement_status(movement_id)
            .unwrap()
            .code
            .as_deref()
            == Some("conflict_result_discard_pending")
        {
            service
                .discard_retained_conflict_result(movement_id)
                .unwrap();
        }
        let status = service.movement_status(movement_id).unwrap();
        assert_eq!(status.state, NativeAgentWorkspaceMovementStateV1::Cancelled);
        assert_eq!(status.code.as_deref(), Some("conflict_result_discarded"));
        assert!(
            crate::storage::get_native_agent_conflict(paths, movement_id)
                .unwrap()
                .is_none()
        );
        assert_ne!(status.state, NativeAgentWorkspaceMovementStateV1::Completed);
        assert_eq!(
            service
                .discard_retained_conflict_result(movement_id)
                .unwrap()
                .code
                .as_deref(),
            Some("conflict_result_discarded")
        );
    }

    #[test]
    fn discard_restart_after_pending_marker_finishes_exact_cleanup() {
        let (root, paths, mut before, conflict) =
            durable_conflict_fixture("movement-discard-marker", "task-discard-marker");
        mark_conflict_discard_pending(&mut before, "movement-discard-marker");
        assert!(conflict.retained_tree.exists());

        let mut after = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert_discarded(&mut after, &paths, "movement-discard-marker");
        assert!(!conflict.retained_tree.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discard_repeat_after_filesystem_delete_finishes_sqlite_then_terminal_fact() {
        let (root, paths, mut service, conflict) =
            durable_conflict_fixture("movement-discard-filesystem", "task-discard-filesystem");
        mark_conflict_discard_pending(&mut service, "movement-discard-filesystem");
        delete_exact_retained_conflict_container(&paths, &conflict).unwrap();
        assert!(!conflict.retained_tree.exists());
        assert!(
            crate::storage::get_native_agent_conflict(&paths, "movement-discard-filesystem")
                .unwrap()
                .is_some()
        );

        assert_discarded(&mut service, &paths, "movement-discard-filesystem");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discard_restart_after_sqlite_delete_finalizes_without_touching_other_results() {
        let (root, paths, mut before, conflict) =
            durable_conflict_fixture("movement-discard-sqlite", "task-discard-sqlite");
        mark_conflict_discard_pending(&mut before, "movement-discard-sqlite");
        delete_exact_retained_conflict_container(&paths, &conflict).unwrap();
        assert!(
            crate::storage::delete_native_agent_conflict(&paths, "movement-discard-sqlite")
                .unwrap()
        );

        let mut after = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert_discarded(&mut after, &paths, "movement-discard-sqlite");
        assert!(!conflict.retained_tree.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn only_one_approved_movement_can_own_a_canonical_source_workspace() {
        let (root, source, executable) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        for (movement, task) in [
            ("movement-owner-a", "task-owner-a"),
            ("movement-owner-b", "task-owner-b"),
        ] {
            service
                .propose_bridge_workspace_movement(
                    "room",
                    movement,
                    task,
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
        }
        let first = service
            .approve_workspace_movement("movement-owner-a", "room", "host:source", &paths.temp_dir)
            .unwrap();
        assert!(service
            .start_codex_task_with_executable_and_id(
                &executable,
                "task-local-overlap",
                &source,
                "must not overlap the approved movement",
            )
            .is_err());
        assert!(service
            .approve_workspace_movement("movement-owner-b", "room", "host:source", &paths.temp_dir)
            .is_err());
        service.interrupt_workspace_movement("movement-owner-a", "outbound_transfer_failed");
        let second = service
            .approve_workspace_movement("movement-owner-b", "room", "host:source", &paths.temp_dir)
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&first.2);
        crate::regular_file_set_transfer::cleanup_package(&second.2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_required_restart_owns_source_until_explicit_cancel() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        for (movement, task) in [
            ("movement-restart-a", "task-restart-a"),
            ("movement-restart-b", "task-restart-b"),
        ] {
            before
                .propose_bridge_workspace_movement(
                    "room",
                    movement,
                    task,
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
        }
        let approved = before
            .approve_workspace_movement(
                "movement-restart-a",
                "room",
                "host:source",
                &paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&approved.2);
        let mut after = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert!(after
            .approve_workspace_movement(
                "movement-restart-b",
                "room",
                "host:source",
                &paths.temp_dir
            )
            .is_err());
        after
            .cancel_remote_task("task-restart-a", "host:remote")
            .unwrap();
        let second = after
            .approve_workspace_movement(
                "movement-restart-b",
                "room",
                "host:source",
                &paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&second.2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restart_before_native_completion_is_interrupted_not_reexecuted() {
        let root = std::env::temp_dir().join(format!("pastey-native-recovery-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .queue_remote_task("task-restart", "host:remote", "/workspace", "task")
            .unwrap();
        let after = NativeAgentServiceV1::with_paths(paths).unwrap();
        let task = after.task_status("task-restart").unwrap();
        assert_eq!(task.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            task.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unbound_local_restart_is_terminal_and_removes_its_unreachable_envelope() {
        let (root, workspace, _) = fixture();
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let task = NativeAgentTaskStatusV1 {
            schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
            task_id: "task-local-restart".into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: "workspace".into(),
            session_reused: false,
            state: NativeAgentTaskStateV1::Running,
            result: None,
            code: None,
            output: None,
            conversation: None,
            native_decisions: Vec::new(),
        };
        before
            .tasks
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), task.clone());
        before
            .task_workspaces
            .insert(task.task_id.clone(), workspace);
        before
            .task_digests
            .insert(task.task_id.clone(), "task-digest".into());
        before.persist_envelope(&task.task_id, None).unwrap();
        assert!(
            crate::storage::get_native_agent_envelope(&paths, &task.task_id)
                .unwrap()
                .is_some()
        );
        drop(before);

        let recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let status = recovered.task_status(&task.task_id).unwrap();
        assert_eq!(status.state, NativeAgentTaskStateV1::Failed);
        assert_eq!(
            status.code.as_deref(),
            Some("native_agent_interrupted_on_restart")
        );
        assert!(
            crate::storage::get_native_agent_envelope(&paths, &task.task_id)
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_duplicate_and_conflicting_task_reuse_are_distinguished_after_restart() {
        let root = std::env::temp_dir().join(format!("pastey-native-replay-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .queue_remote_task("task-replay", "host:remote", "/workspace", "task")
            .unwrap();
        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(
            after
                .queue_remote_task("task-replay", "host:remote", "/workspace", "task")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Interrupted
        );
        assert!(after
            .queue_remote_task("task-replay", "host:other", "/workspace", "task")
            .is_err());
        assert!(after
            .queue_remote_task("task-replay", "host:remote", "/other", "task")
            .is_err());
        assert!(after
            .queue_remote_task("task-replay", "host:remote", "/other/workspace", "task")
            .is_err());
        assert!(after
            .queue_remote_task("task-replay", "host:remote", "/workspace", "other task")
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invoke_delivery_receipt_loss_requires_reconciliation_not_failure() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-invoke-loss-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .queue_remote_task("task-invoke-loss", "host:remote", "/workspace", "task")
            .unwrap();
        service.fail_remote_delivery("task-invoke-loss").unwrap();
        let task = service.task_status("task-invoke-loss").unwrap();
        assert_eq!(task.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            task.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        let restarted = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(
            restarted.task_status("task-invoke-loss").unwrap().state,
            NativeAgentTaskStateV1::Interrupted
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_prepare_is_durable_idempotent_and_conflicting_reuse_fails_closed() {
        let root = std::env::temp_dir().join(format!("pastey-native-prepare-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let request = NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-prepare".into(),
            task_id: "task-prepare".into(),
            source_host_ref: "host:source".into(),
            target_host_ref: "host:target".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: "a".repeat(64),
            source_bytes: 1,
        };
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before.accept_workspace_prepare(request.clone()).unwrap();
        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert!(after.accept_workspace_prepare(request.clone()).is_ok());
        let mut conflicting = request;
        conflicting.task = "different".into();
        assert!(after.accept_workspace_prepare(conflicting).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_workspace_prepare_keeps_immutable_correlation_after_outbound_landing() {
        let (root, source, agent) = fixture();
        fs::write(source.join("input.txt"), b"outbound input").unwrap();
        let paths = durable_paths(&root);
        let source_identity = crate::safe_file_identity::capture_regular_file_set_identity(
            &source,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        let request = NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-landing".into(),
            task_id: "task-landing".into(),
            source_host_ref: "host:source".into(),
            target_host_ref: "host:target".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: source_identity.digest.clone(),
            source_bytes: source_identity.byte_count,
        };
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from("room-landing", "host:source", request.clone())
            .unwrap();
        let before = crate::storage::get_native_agent_envelope(&paths, "task-landing")
            .unwrap()
            .unwrap()
            .immutable_correlation;
        let package = crate::regular_file_set_transfer::prepare_package(
            &source,
            &source,
            &source_identity,
            &paths.temp_dir,
        )
        .unwrap();
        let landed = crate::regular_file_set_transfer::materialize_package(
            &package,
            &paths.temp_dir,
            &source_identity.digest,
            source_identity.byte_count,
        )
        .unwrap();
        land_and_accept(&mut service, &agent, "movement-landing", &landed).unwrap();
        let after = crate::storage::get_native_agent_envelope(&paths, "task-landing")
            .unwrap()
            .unwrap()
            .immutable_correlation;
        assert_eq!(before, after);
        crate::regular_file_set_transfer::cleanup_package(&package);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completed_apply_is_durable_and_duplicate_return_is_a_noop() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("after.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .propose_workspace_movement(
                "movement-apply",
                "task-apply",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        service
            .workspace_movements
            .get_mut("movement-apply")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        let first = service
            .apply_received_workspace_return("movement-apply", &returned)
            .unwrap();
        assert_eq!(first.state, NativeAgentWorkspaceMovementStateV1::Completed);
        let mut again = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(
            again.movement_status("movement-apply").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert_eq!(
            again
                .apply_received_workspace_return("movement-apply", &returned)
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert_eq!(
            fs::read(source.join("after.txt")).unwrap(),
            b"native result"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_crash_restart_at_every_boundary_restores_canonical_workspace_and_exact_retry() {
        let crash_points = [
            ApplyCrashPointV1::IntentPersisted,
            ApplyCrashPointV1::StageWritten,
            ApplyCrashPointV1::StageJournaled,
            ApplyCrashPointV1::OriginalMoved,
            ApplyCrashPointV1::OriginalMoveJournaled,
            ApplyCrashPointV1::ResultInstalled,
            ApplyCrashPointV1::ResultInstallJournaled,
            ApplyCrashPointV1::Committed,
            ApplyCrashPointV1::BackupCleaned,
        ];

        for (index, crash_point) in crash_points.into_iter().enumerate() {
            let (root, source, _) = fixture();
            let movement_id = format!("movement-apply-crash-{index}");
            let task_id = format!("task-apply-crash-{index}");
            fs::write(source.join("baseline.txt"), b"approved baseline").unwrap();
            let returned = root.join("returned");
            fs::create_dir(&returned).unwrap();
            fs::write(returned.join("result.txt"), b"exact native result").unwrap();
            let returned_identity = crate::safe_file_identity::capture_regular_file_set_identity(
                &returned,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )
            .unwrap();
            let paths = durable_paths(&root);
            let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            before
                .propose_bridge_workspace_movement(
                    "room-apply-crash",
                    &movement_id,
                    &task_id,
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit the workspace",
                    true,
                )
                .unwrap();
            let record = before.workspace_movements.get_mut(&movement_id).unwrap();
            record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
            let source_record = record.source.clone().unwrap();
            before.persist_envelope(&task_id, None).unwrap();

            assert!(before
                .apply_exact_workspace_result_with_crash(
                    &movement_id,
                    &source_record,
                    &returned,
                    returned_identity.clone(),
                    Some(crash_point),
                )
                .is_err());
            drop(before);

            let mut recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            assert!(
                source.is_dir(),
                "canonical source missing at {crash_point:?}"
            );
            let recovered_identity = crate::safe_file_identity::capture_regular_file_set_identity(
                &source,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )
            .unwrap();
            let before_stage = source_record.baseline == recovered_identity;
            let is_intent_only = crash_point == ApplyCrashPointV1::IntentPersisted;
            assert_eq!(before_stage, is_intent_only, "boundary {crash_point:?}");

            let status = recovered.movement_status(&movement_id).unwrap();
            if is_intent_only {
                assert_eq!(
                    status.state,
                    NativeAgentWorkspaceMovementStateV1::Interrupted
                );
                assert_eq!(status.code.as_deref(), Some("result_apply_interrupted"));
                let transfer = NativeAgentWorkspaceTransferV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: movement_id.clone(),
                    task_id: task_id.clone(),
                    phase: NativeAgentWorkspaceTransferPhaseV1::Return,
                    bridge_id: "room-apply-crash".into(),
                    source_host_ref: "host:remote".into(),
                    destination_host_ref: "host:source".into(),
                    object: movement_object(),
                    content_digest: returned_identity.digest.clone(),
                    logical_byte_count: returned_identity.byte_count,
                };
                recovered
                    .validate_workspace_transfer(&transfer, "host:source")
                    .unwrap();
                recovered
                    .authorize_source_apply_retry("room-apply-crash", &movement_id)
                    .unwrap();
                assert_eq!(
                    recovered
                        .apply_received_workspace_return(&movement_id, &returned)
                        .unwrap()
                        .state,
                    NativeAgentWorkspaceMovementStateV1::Completed
                );
            } else {
                assert_eq!(status.state, NativeAgentWorkspaceMovementStateV1::Completed);
            }

            assert_eq!(
                crate::safe_file_identity::capture_regular_file_set_identity(
                    &source,
                    crate::storage::MAX_FILE_SIZE_BYTES,
                )
                .unwrap()
                .digest,
                returned_identity.digest,
                "exact Return must be installed at {crash_point:?}"
            );
            let (stage, backup) = apply_transaction_paths(&source, &movement_id).unwrap();
            assert!(!stage.exists(), "stage leaked at {crash_point:?}");
            assert!(!backup.exists(), "backup leaked at {crash_point:?}");

            fs::write(source.join("post-commit-user-edit.txt"), b"local edit").unwrap();
            assert_eq!(
                recovered
                    .apply_received_workspace_return(&movement_id, &returned)
                    .unwrap()
                    .state,
                NativeAgentWorkspaceMovementStateV1::Completed
            );
            assert_eq!(
                fs::read(source.join("post-commit-user-edit.txt")).unwrap(),
                b"local edit",
                "duplicate exact Return must not apply over later user edits"
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn burned_bridge_with_unprovable_apply_journal_finalizes_on_every_startup() {
        for in_memory_purge in [false, true] {
            let (root, source, _) = fixture();
            fs::write(source.join("baseline.txt"), b"approved baseline").unwrap();
            let returned = root.join("returned");
            fs::create_dir(&returned).unwrap();
            fs::write(returned.join("result.txt"), b"exact native result").unwrap();
            let identity = crate::safe_file_identity::capture_regular_file_set_identity(
                &returned,
                crate::storage::MAX_FILE_SIZE_BYTES,
            )
            .unwrap();
            let paths = durable_paths(&root);
            crate::storage::create_room(
                &paths,
                &[9u8; 32],
                "123456",
                30,
                crate::models::LocalRole::Creator,
                Some("room-unsafe-apply".into()),
                None,
            )
            .unwrap();
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            service
                .propose_bridge_workspace_movement(
                    "room-unsafe-apply",
                    "movement-unsafe-apply",
                    "task-unsafe-apply",
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
            let source_record = {
                let record = service
                    .workspace_movements
                    .get_mut("movement-unsafe-apply")
                    .unwrap();
                record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
                record.source.clone().unwrap()
            };
            service
                .tasks
                .lock()
                .unwrap()
                .get_mut("task-unsafe-apply")
                .unwrap()
                .state = NativeAgentTaskStateV1::Completed;
            service.persist_envelope("task-unsafe-apply", None).unwrap();
            assert!(service
                .apply_exact_workspace_result_with_crash(
                    "movement-unsafe-apply",
                    &source_record,
                    &returned,
                    identity,
                    Some(ApplyCrashPointV1::StageJournaled),
                )
                .is_err());
            let (stage, backup) =
                apply_transaction_paths(&source, "movement-unsafe-apply").unwrap();
            assert!(stage.is_dir());
            assert!(!backup.exists());
            fs::write(source.join("user-change.txt"), b"unexpected user content").unwrap();
            assert!(recover_workspace_apply_record(
                "movement-unsafe-apply",
                service
                    .workspace_movements
                    .get_mut("movement-unsafe-apply")
                    .unwrap(),
            )
            .is_err());
            assert_eq!(
                fs::read(source.join("user-change.txt")).unwrap(),
                b"unexpected user content"
            );
            crate::storage::cut_off_bridge_authority(&paths, "room-unsafe-apply").unwrap();
            assert!(crate::storage::is_burned_bridge(&paths, "room-unsafe-apply").unwrap());
            if in_memory_purge {
                service.purge_bridge_authority("room-unsafe-apply").unwrap();
                assert!(service
                    .propose_bridge_workspace_movement(
                        "room-unsafe-apply",
                        "movement-late",
                        "task-late",
                        &source,
                        "host:remote",
                        movement_object(),
                        "late",
                        true,
                    )
                    .is_err());
            }
            drop(service);
            for _ in 0..2 {
                crate::storage::finalize_burned_room(&paths, "room-unsafe-apply", &paths.inbox_dir)
                    .unwrap();
                assert!(crate::storage::is_burned_bridge(&paths, "room-unsafe-apply").unwrap());
                assert!(crate::storage::list_native_agent_envelopes(&paths)
                    .unwrap()
                    .is_empty());
                assert_eq!(
                    fs::read(source.join("user-change.txt")).unwrap(),
                    b"unexpected user content"
                );
                assert!(stage.is_dir());
                assert!(!backup.exists());
                let restored = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
                assert!(restored.movement_status("movement-unsafe-apply").is_err());
            }
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn session_loss_retains_durable_native_recovery_material_but_burn_removes_it() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-session-loss-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let task_workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "session-loss-task",
        )
        .unwrap();
        fs::write(
            task_workspace.join("workspace.txt"),
            b"remote task workspace",
        )
        .unwrap();
        let (snapshot, identity) =
            snapshot_exact_workspace_result(&paths, "movement-session-loss", &task_workspace)
                .unwrap();
        let request = NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-session-loss".into(),
            task_id: "task-session-loss".into(),
            source_host_ref: "host:source".into(),
            target_host_ref: "host:local".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: "b".repeat(64),
            source_bytes: 1,
        };
        let runtime = crate::host_runtime::HostRuntime::new(
            paths.clone(),
            test_config(),
            Arc::new(NoopEventSink),
            Arc::new(NoopTaskSpawner),
        )
        .unwrap();
        runtime
            .native_agents
            .lock()
            .accept_bridge_workspace_prepare_from(
                "room-session-loss",
                &request.source_host_ref.clone(),
                request,
            )
            .unwrap();
        {
            let mut native_agents = runtime.native_agents.lock();
            let movement = native_agents
                .workspace_movements
                .get_mut("movement-session-loss")
                .unwrap();
            movement.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
            movement.task_workspace = Some(task_workspace.clone());
            movement.result_snapshot = Some(snapshot.clone());
            movement.result_identity = Some(identity);
        }
        {
            let native_agents = runtime.native_agents.lock();
            let mut tasks = native_agents.tasks.lock().unwrap();
            let task = tasks.get_mut("task-session-loss").unwrap();
            task.state = NativeAgentTaskStateV1::Running;
            task.code = None;
        }
        runtime
            .native_agents
            .lock()
            .persist_envelope("task-session-loss", None)
            .unwrap();

        runtime.purge_room("room-session-loss");
        let native_agents = runtime.native_agents.lock();
        assert!(!native_agents.revoked_bridges.contains("room-session-loss"));
        assert!(snapshot.exists());
        assert!(task_workspace.exists());
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-session-loss")
                .unwrap()
                .is_some()
        );
        drop(native_agents);
        drop(runtime);

        let mut recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let projection = recovered
            .recovery_projection_for_bridge("room-session-loss")
            .unwrap()
            .unwrap();
        assert_eq!(projection.task.task_id, "task-session-loss");
        assert_eq!(
            projection
                .movement
                .as_ref()
                .map(|movement| movement.target_host_ref.as_str()),
            Some("host:local")
        );
        assert_eq!(
            projection
                .movement
                .as_ref()
                .map(|movement| movement.state.clone()),
            Some(NativeAgentWorkspaceMovementStateV1::Interrupted)
        );
        assert_eq!(
            projection.task.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        assert!(snapshot.exists());
        assert!(task_workspace.exists());

        recovered
            .purge_bridge_authority("room-session-loss")
            .unwrap();
        assert!(recovered.revoked_bridges.contains("room-session-loss"));
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-session-loss")
                .unwrap()
                .is_none()
        );
        assert!(!snapshot.exists());
        assert!(!task_workspace.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transient_session_loss_keeps_a_live_native_turn_observed_without_interrupting_it() {
        let (root, workspace, agent) = blocked_turn_fixture();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths).unwrap();
        let started = start_accepted_bridge_codex_task(
            &mut service,
            "room-live-turn",
            &agent,
            "task-live-turn",
            &workspace,
            "finish this turn after the route is lost",
        )
        .unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();
        let controller = service
            .codex
            .wait_for_session_controller(&canonical_workspace);
        for _ in 0..300 {
            if controller.active_turn.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.active_turn.lock().unwrap().is_some());
        wait_for_file(&workspace.join("turn-started"));
        assert_eq!(
            service.workspace_occupant(&canonical_workspace).as_deref(),
            Some(started.task_id.as_str())
        );

        service.revoke_bridge_session("room-live-turn").unwrap();

        assert_eq!(
            service.task_status("task-live-turn").unwrap().state,
            NativeAgentTaskStateV1::Running
        );
        assert!(service.codex.has_session(&canonical_workspace));
        assert!(!controller.interrupt_requested.load(Ordering::Acquire));
        fs::write(workspace.join("release-turn"), b"continue").unwrap();
        assert_eq!(
            wait_for_terminal(&service, "task-live-turn").state,
            NativeAgentTaskStateV1::Completed
        );
        for _ in 0..100 {
            if workspace.join("terminal-sent").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(50));
        assert!(!workspace.join("turn-interrupt-requested").exists());
        assert!(service.codex.has_session(&canonical_workspace));
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completed_exact_result_remains_return_retryable_after_session_loss_and_restart() {
        let root = std::env::temp_dir().join(format!(
            "pastey-native-completed-session-loss-{}",
            Uuid::new_v4()
        ));
        let paths = durable_paths(&root);
        let task_workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "completed-session-loss-task",
        )
        .unwrap();
        fs::write(task_workspace.join("workspace.txt"), b"completed result").unwrap();
        let (snapshot, identity) = snapshot_exact_workspace_result(
            &paths,
            "movement-completed-session-loss",
            &task_workspace,
        )
        .unwrap();
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                "room-completed-session-loss",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-completed-session-loss".into(),
                    task_id: "task-completed-session-loss".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        {
            let record = service
                .workspace_movements
                .get_mut("movement-completed-session-loss")
                .unwrap();
            record.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
            record.task_workspace = Some(task_workspace.clone());
            record.result_snapshot = Some(snapshot.clone());
            record.result_identity = Some(identity);
            let mut tasks = service.tasks.lock().unwrap();
            let task = tasks.get_mut("task-completed-session-loss").unwrap();
            task.state = NativeAgentTaskStateV1::Completed;
            task.code = None;
        }
        service
            .persist_envelope("task-completed-session-loss", None)
            .unwrap();

        service
            .revoke_bridge_session("room-completed-session-loss")
            .unwrap();
        let after_loss = service
            .movement_status("movement-completed-session-loss")
            .unwrap();
        assert_eq!(
            after_loss.state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        assert_eq!(
            after_loss.code.as_deref(),
            Some("result_return_retry_required")
        );
        assert_eq!(
            service
                .task_status("task-completed-session-loss")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Completed
        );
        drop(service);

        let mut recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let after_restart = recovered
            .movement_status("movement-completed-session-loss")
            .unwrap();
        assert_eq!(
            after_restart.state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        assert_eq!(
            after_restart.code.as_deref(),
            Some("result_return_retry_required")
        );
        let request = NativeAgentRetryResultReturnV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            retry_id: "fresh-session-retry".into(),
            movement_id: "movement-completed-session-loss".into(),
            task_id: "task-completed-session-loss".into(),
            target_host_ref: "host:local".into(),
        };
        recovered
            .authorize_result_return_retry(
                "room-completed-session-loss",
                &request,
                "host:source",
                "host:local",
            )
            .unwrap();
        assert!(snapshot.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completed_received_task_recovers_missing_result_snapshot_after_restart_without_rerun() {
        let (root, _, agent) = fixture_with_turn_start_behavior(
            r#"echo run >> "$PWD/agent-runs"; echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'; sleep 0.1; echo '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'"#,
        );
        let paths = durable_paths(&root);
        let task_workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "completed-before-snapshot",
        )
        .unwrap();
        fs::write(task_workspace.join("result.txt"), b"completed Agent result").unwrap();
        let task_id = "task-completed-before-snapshot";
        let movement_id = "movement-completed-before-snapshot";
        let bridge_id = "room-completed-before-snapshot";
        let prompt = "complete remotely";
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                bridge_id,
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: movement_id.into(),
                    task_id: task_id.into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: prompt.into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        let started = land_and_accept(&mut service, &agent, movement_id, &task_workspace).unwrap();
        assert_eq!(started.state, NativeAgentTaskStateV1::Running);
        assert_eq!(
            wait_for_terminal(&service, task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        for _ in 0..300 {
            let durable = crate::storage::get_native_agent_envelope(&paths, task_id)
                .unwrap()
                .unwrap();
            let persisted: PersistedNativeAgentEnvelopeV1 =
                serde_json::from_str(&durable.record_json).unwrap();
            if persisted.task.state == NativeAgentTaskStateV1::Completed {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs::read_to_string(task_workspace.join("agent-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        {
            let movement = service.workspace_movements.get(movement_id).unwrap();
            assert!(movement.result_snapshot.is_none());
            assert!(movement.result_identity.is_none());
            let durable = crate::storage::get_native_agent_envelope(&paths, task_id)
                .unwrap()
                .unwrap();
            let persisted: PersistedNativeAgentEnvelopeV1 =
                serde_json::from_str(&durable.record_json).unwrap();
            assert_eq!(persisted.task.state, NativeAgentTaskStateV1::Completed);
            assert!(persisted
                .movement
                .as_ref()
                .unwrap()
                .result_snapshot
                .is_none());
        }

        service.revoke_bridge_session(bridge_id).unwrap();
        let after_session_loss = service.movement_status(movement_id).unwrap();
        assert_eq!(
            after_session_loss.state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        assert_eq!(
            after_session_loss.code.as_deref(),
            Some("result_snapshot_capture_pending")
        );
        assert_eq!(
            service.task_status(task_id).unwrap().state,
            NativeAgentTaskStateV1::Completed
        );
        service.shutdown();
        drop(service);

        let mut recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let after_restart = recovered.movement_status(movement_id).unwrap();
        assert_eq!(
            after_restart.state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        assert_eq!(
            after_restart.code.as_deref(),
            Some("result_return_retry_required")
        );
        assert_eq!(
            recovered.task_status(task_id).unwrap().state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(recovered.codex.session_count(), 0);
        let movement = recovered.workspace_movements.get(movement_id).unwrap();
        let snapshot = movement.result_snapshot.as_ref().unwrap();
        let identity = movement.result_identity.as_ref().unwrap();
        let expected = crate::safe_file_identity::capture_regular_file_set_identity(
            &task_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        assert!(same_logical_file_set(identity, &expected));
        assert!(validate_exact_app_owned_result(&paths, snapshot, identity).is_ok());
        let request = NativeAgentRetryResultReturnV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            retry_id: "fresh-session-result-retry".into(),
            movement_id: movement_id.into(),
            task_id: task_id.into(),
            target_host_ref: "host:local".into(),
        };
        recovered
            .authorize_result_return_retry(bridge_id, &request, "host:source", "host:local")
            .unwrap();
        assert_eq!(
            fs::read_to_string(task_workspace.join("agent-runs"))
                .unwrap()
                .lines()
                .count(),
            1,
            "restart recovery seals the completed tree without rerunning Codex"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completed_result_snapshot_recovery_failure_and_revocation_do_not_revive_execution() {
        let root = std::env::temp_dir().join(format!(
            "pastey-native-result-recovery-fences-{}",
            Uuid::new_v4()
        ));
        let paths = durable_paths(&root);

        let missing_workspace = paths.temp_dir.join("missing-completed-task");
        let missing = completed_received_workspace_service(
            &paths,
            "room-missing-result",
            "movement-missing-result",
            "task-missing-result",
            &missing_workspace,
        );
        drop(missing);
        let failed = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let failure = failed.movement_status("movement-missing-result").unwrap();
        assert_eq!(failure.state, NativeAgentWorkspaceMovementStateV1::Failed);
        assert_eq!(
            failure.code.as_deref(),
            Some("native_agent_result_snapshot_recovery_failed")
        );
        assert_eq!(
            failed.task_status("task-missing-result").unwrap().state,
            NativeAgentTaskStateV1::Completed
        );
        drop(failed);

        let cancelled_workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "cancelled-result-task",
        )
        .unwrap();
        fs::write(cancelled_workspace.join("result.txt"), b"completed result").unwrap();
        let mut cancelled = completed_received_workspace_service(
            &paths,
            "room-cancelled-result",
            "movement-cancelled-result",
            "task-cancelled-result",
            &cancelled_workspace,
        );
        {
            let movement = cancelled
                .workspace_movements
                .get_mut("movement-cancelled-result")
                .unwrap();
            movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
            movement.status.code = Some("native_agent_reconciliation_required".into());
        }
        cancelled
            .persist_envelope("task-cancelled-result", None)
            .unwrap();
        cancelled
            .stop_bridge_task_authority("room-cancelled-result", "task-cancelled-result")
            .unwrap();
        drop(cancelled);
        let cancelled = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert_eq!(
            cancelled
                .movement_status("movement-cancelled-result")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        assert_eq!(
            cancelled
                .task_status("task-cancelled-result")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Completed
        );
        assert!(cancelled
            .workspace_movements
            .get("movement-cancelled-result")
            .unwrap()
            .result_snapshot
            .is_none());
        assert!(cancelled_workspace.exists());
        drop(cancelled);

        let burned_workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "burned-result-task",
        )
        .unwrap();
        fs::write(burned_workspace.join("result.txt"), b"completed result").unwrap();
        let mut burned = completed_received_workspace_service(
            &paths,
            "room-burned-result",
            "movement-burned-result",
            "task-burned-result",
            &burned_workspace,
        );
        burned.purge_bridge_authority("room-burned-result").unwrap();
        assert!(!burned_workspace.exists());
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-burned-result")
                .unwrap()
                .is_none()
        );
        drop(burned);
        let after_burn = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert!(after_burn
            .movement_status("movement-burned-result")
            .is_err());
        assert!(after_burn.task_status("task-burned-result").is_err());
        let snapshots = paths.app_data_dir.join("native-agent-results");
        assert!(!snapshots.exists() || fs::read_dir(snapshots).unwrap().next().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn receiver_artifacts_are_cleaned_after_return_terminal_failure_cancel_and_restart() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-artifact-cleanup-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();

        let prepare = |movement_id: &str, task_id: &str| NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id: task_id.into(),
            source_host_ref: "host:source".into(),
            target_host_ref: "host:local".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: "b".repeat(64),
            source_bytes: 1,
        };
        let attach_private_artifacts =
            |service: &mut NativeAgentServiceV1, movement_id: &str, task_id: &str, unique: &str| {
                let workspace = crate::safe_file_identity::create_private_tree_root(
                    &paths.temp_dir,
                    "native-v2-file-sets",
                    unique,
                )
                .unwrap();
                fs::write(workspace.join("result.txt"), b"app-owned task result").unwrap();
                let (snapshot, identity) =
                    snapshot_exact_workspace_result(&paths, movement_id, &workspace).unwrap();
                let movement = service.workspace_movements.get_mut(movement_id).unwrap();
                movement.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
                movement.task_workspace = Some(workspace.clone());
                movement.result_snapshot = Some(snapshot.clone());
                movement.result_identity = Some(identity);
                service
                    .task_workspaces
                    .insert(task_id.to_owned(), workspace.clone());
                service.persist_envelope(task_id, None).unwrap();
                (workspace, snapshot)
            };

        service
            .accept_bridge_workspace_prepare_from(
                "room-artifacts",
                "host:source",
                prepare("movement-return-cleanup", "task-return-cleanup"),
            )
            .unwrap();
        let (returned_workspace, returned_snapshot) = attach_private_artifacts(
            &mut service,
            "movement-return-cleanup",
            "task-return-cleanup",
            "return-cleanup-task",
        );
        service
            .tasks
            .lock()
            .unwrap()
            .get_mut("task-return-cleanup")
            .unwrap()
            .state = NativeAgentTaskStateV1::Completed;
        service
            .workspace_movements
            .get_mut("movement-return-cleanup")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        service
            .persist_envelope("task-return-cleanup", None)
            .unwrap();
        service
            .complete_workspace_result_return("movement-return-cleanup")
            .unwrap();
        assert!(!returned_workspace.exists());
        assert!(!returned_snapshot.exists());

        service
            .accept_bridge_workspace_prepare_from(
                "room-artifacts",
                "host:source",
                prepare("movement-restart-cleanup", "task-restart-cleanup"),
            )
            .unwrap();
        let (restart_workspace, restart_snapshot) = attach_private_artifacts(
            &mut service,
            "movement-restart-cleanup",
            "task-restart-cleanup",
            "restart-cleanup-task",
        );
        service
            .tasks
            .lock()
            .unwrap()
            .get_mut("task-restart-cleanup")
            .unwrap()
            .state = NativeAgentTaskStateV1::Completed;
        {
            let movement = service
                .workspace_movements
                .get_mut("movement-restart-cleanup")
                .unwrap();
            movement.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
            movement.apply_completed = true;
        }
        service
            .persist_envelope("task-restart-cleanup", None)
            .unwrap();
        drop(service);

        let recovered = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        assert!(!restart_workspace.exists());
        assert!(!restart_snapshot.exists());
        assert!(recovered
            .workspace_movements
            .get("movement-restart-cleanup")
            .unwrap()
            .task_workspace
            .is_none());
        drop(recovered);

        for (movement_id, task_id, unique, terminal) in [
            (
                "movement-failed-cleanup",
                "task-failed-cleanup",
                "failed-cleanup-task",
                NativeAgentTaskStateV1::Failed,
            ),
            (
                "movement-cancelled-cleanup",
                "task-cancelled-cleanup",
                "cancelled-cleanup-task",
                NativeAgentTaskStateV1::Cancelled,
            ),
        ] {
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            service
                .accept_bridge_workspace_prepare_from(
                    "room-artifacts",
                    "host:source",
                    prepare(movement_id, task_id),
                )
                .unwrap();
            let (task_workspace, snapshot) =
                attach_private_artifacts(&mut service, movement_id, task_id, unique);
            service
                .tasks
                .lock()
                .unwrap()
                .get_mut(task_id)
                .unwrap()
                .state = terminal.clone();
            service.persist_envelope(task_id, None).unwrap();
            assert!(service
                .cleanup_terminal_remote_workspace(movement_id)
                .unwrap());
            assert!(!task_workspace.exists());
            assert!(!snapshot.exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_return_transfer_landing_is_acknowledged_without_reapplying() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("after.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let runtime = crate::host_runtime::HostRuntime::new(
            paths.clone(),
            test_config(),
            Arc::new(NoopEventSink),
            Arc::new(NoopTaskSpawner),
        )
        .unwrap();
        runtime
            .native_agents
            .lock()
            .propose_bridge_workspace_movement(
                "room",
                "movement-return-landing",
                "task-return-landing",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        runtime
            .native_agents
            .lock()
            .workspace_movements
            .get_mut("movement-return-landing")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        let identity = crate::safe_file_identity::capture_regular_file_set_identity(
            &returned,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        let metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-return-landing".into(),
            task_id: "task-return-landing".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room".into(),
            source_host_ref: "host:remote".into(),
            destination_host_ref: runtime.local_host_ref.as_str().into(),
            object: movement_object(),
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        };
        let receive_return = |suffix: &str| {
            let prepared = crate::regular_file_set_transfer::prepare_package(
                &returned,
                &returned,
                &identity,
                &paths.temp_dir,
            )
            .unwrap();
            let receive_root = paths
                .temp_dir
                .join("native-agent-workspace-transfers")
                .join(suffix);
            fs::create_dir_all(&receive_root).unwrap();
            let received = receive_root.join("package");
            fs::copy(&prepared, &received).unwrap();
            crate::regular_file_set_transfer::cleanup_package(&prepared);
            (received, receive_root)
        };
        let (first, first_receive_root) = receive_return("return-first");
        assert_eq!(
            register_workspace_transfer_landing(
                &runtime,
                &metadata,
                first,
                crate::storage::now_ts()
            )
            .unwrap()
            .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert!(!first_receive_root.exists());
        let materialized_root = paths.temp_dir.join("native-v2-file-sets");
        assert!(
            !materialized_root.exists()
                || fs::read_dir(&materialized_root).unwrap().next().is_none(),
            "successful Return must remove its materialized tree"
        );
        let (duplicate, duplicate_receive_root) = receive_return("return-duplicate");
        assert_eq!(
            register_workspace_transfer_landing(
                &runtime,
                &metadata,
                duplicate,
                crate::storage::now_ts()
            )
            .unwrap()
            .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert!(!duplicate_receive_root.exists());
        assert!(
            !materialized_root.exists()
                || fs::read_dir(&materialized_root).unwrap().next().is_none(),
            "duplicate Return acknowledgement must not leave a materialized tree"
        );
        let mut conflicting = metadata.clone();
        conflicting.content_digest = "f".repeat(64);
        assert!(runtime
            .native_agents
            .lock()
            .validate_workspace_transfer(&conflicting, runtime.local_host_ref.as_str())
            .is_err());
        assert_eq!(
            fs::read(source.join("after.txt")).unwrap(),
            b"native result"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_conflict_return_landing_is_acknowledged_without_second_retention() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"approved baseline").unwrap();
        let returned = root.join("returned");
        fs::create_dir(&returned).unwrap();
        fs::write(returned.join("after.txt"), b"native result").unwrap();
        let paths = durable_paths(&root);
        let runtime = crate::host_runtime::HostRuntime::new(
            paths.clone(),
            test_config(),
            Arc::new(NoopEventSink),
            Arc::new(NoopTaskSpawner),
        )
        .unwrap();
        runtime
            .native_agents
            .lock()
            .propose_bridge_workspace_movement(
                "room",
                "movement-conflict-landing",
                "task-conflict-landing",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        runtime
            .native_agents
            .lock()
            .workspace_movements
            .get_mut("movement-conflict-landing")
            .unwrap()
            .status
            .state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        fs::write(source.join("local-change.txt"), b"local").unwrap();
        let identity = crate::safe_file_identity::capture_regular_file_set_identity(
            &returned,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        let metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-conflict-landing".into(),
            task_id: "task-conflict-landing".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room".into(),
            source_host_ref: "host:remote".into(),
            destination_host_ref: runtime.local_host_ref.as_str().into(),
            object: movement_object(),
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        };
        for _ in 0..2 {
            let package = crate::regular_file_set_transfer::prepare_package(
                &returned,
                &returned,
                &identity,
                &paths.temp_dir,
            )
            .unwrap();
            assert_eq!(
                register_workspace_transfer_landing(
                    &runtime,
                    &metadata,
                    package,
                    crate::storage::now_ts()
                )
                .unwrap()
                .state,
                NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired,
            );
        }
        let retained =
            crate::storage::get_native_agent_conflict(&paths, "movement-conflict-landing")
                .unwrap()
                .unwrap();
        assert_eq!(retained.result_digest, identity.digest);
        assert_eq!(
            fs::read(source.join("before.txt")).unwrap(),
            b"approved baseline"
        );
        let mut conflicting = metadata.clone();
        conflicting.content_digest = "f".repeat(64);
        assert!(runtime
            .native_agents
            .lock()
            .validate_workspace_transfer(&conflicting, runtime.local_host_ref.as_str())
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_result_snapshot_survives_restart_for_return_retry() {
        let (root, workspace, _) = fixture();
        fs::write(workspace.join("result.txt"), b"stable result").unwrap();
        let paths = durable_paths(&root);
        let request = NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-result".into(),
            task_id: "task-result".into(),
            source_host_ref: "host:source".into(),
            target_host_ref: "host:target".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: "b".repeat(64),
            source_bytes: 13,
        };
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before.accept_workspace_prepare(request).unwrap();
        let record = before
            .workspace_movements
            .get_mut("movement-result")
            .unwrap();
        record.task_workspace = Some(workspace.clone());
        before
            .tasks
            .lock()
            .unwrap()
            .get_mut("task-result")
            .unwrap()
            .state = NativeAgentTaskStateV1::Completed;
        before.persist_envelope("task-result", None).unwrap();
        let (_, snapshot, _, identity) = before
            .captured_result_snapshot_for_return("movement-result")
            .unwrap();
        assert!(snapshot.starts_with(paths.app_data_dir.join("native-agent-results")));
        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        let (_, retried, _, retried_identity) = after
            .captured_result_snapshot_for_return("movement-result")
            .unwrap();
        assert_eq!(retried, snapshot);
        assert_eq!(retried_identity, identity);
        assert_eq!(
            after.movement_status("movement-result").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn executing_host_return_retry_requires_exact_bridge_session_and_snapshot() {
        let (root, workspace, _) = fixture();
        fs::write(workspace.join("result.txt"), b"exact result").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                "room-retry",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-retry".into(),
                    task_id: "task-retry".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        let (snapshot, identity) =
            snapshot_exact_workspace_result(&paths, "movement-retry", &workspace).unwrap();
        let movement = service
            .workspace_movements
            .get_mut("movement-retry")
            .unwrap();
        movement.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
        movement.status.code = Some("result_return_retry_required".into());
        movement.result_snapshot = Some(snapshot.clone());
        movement.result_identity = Some(identity);
        service
            .tasks
            .lock()
            .unwrap()
            .get_mut("task-retry")
            .unwrap()
            .state = NativeAgentTaskStateV1::Completed;
        service.persist_envelope("task-retry", None).unwrap();

        let request = NativeAgentRetryResultReturnV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            retry_id: "retry-1".into(),
            movement_id: "movement-retry".into(),
            task_id: "task-retry".into(),
            target_host_ref: "host:local".into(),
        };
        service
            .authorize_result_return_retry("room-retry", &request, "host:source", "host:local")
            .unwrap();
        assert!(service
            .authorize_result_return_retry("room-retry", &request, "host:replaced", "host:local")
            .is_err());
        let mut wrong_bridge = request.clone();
        wrong_bridge.target_host_ref = "host:other".into();
        assert!(service
            .authorize_result_return_retry("room-retry", &wrong_bridge, "host:source", "host:local")
            .is_err());

        fs::write(&snapshot.join("result.txt"), b"tampered result").unwrap();
        assert!(service
            .authorize_result_return_retry("room-retry", &request, "host:source", "host:local")
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn result_return_retry_cleans_prepared_package_when_current_route_is_unavailable() {
        let (root, workspace, _) = fixture();
        fs::write(workspace.join("result.txt"), b"durable result").unwrap();
        let paths = durable_paths(&root);
        let runtime = Arc::new(
            crate::host_runtime::HostRuntime::new(
                paths.clone(),
                test_config(),
                Arc::new(NoopEventSink),
                Arc::new(NoopTaskSpawner),
            )
            .unwrap(),
        );
        runtime
            .native_agents
            .lock()
            .accept_bridge_workspace_prepare_from(
                "room-no-route",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-no-route".into(),
                    task_id: "task-no-route".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: runtime.local_host_ref.as_str().into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        let (snapshot, identity) =
            snapshot_exact_workspace_result(&paths, "movement-no-route", &workspace).unwrap();
        {
            let mut service = runtime.native_agents.lock();
            let movement = service
                .workspace_movements
                .get_mut("movement-no-route")
                .unwrap();
            movement.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
            movement.status.code = Some("result_return_retry_required".into());
            movement.result_snapshot = Some(snapshot.clone());
            movement.result_identity = Some(identity);
            service
                .tasks
                .lock()
                .unwrap()
                .get_mut("task-no-route")
                .unwrap()
                .state = NativeAgentTaskStateV1::Completed;
            service.persist_envelope("task-no-route", None).unwrap();
        }

        assert!(retry_workspace_result_return(
            runtime.clone(),
            "room-no-route",
            "movement-no-route",
        )
        .await
        .is_err());
        let packages = paths.temp_dir.join("native-v2-transfer-packages");
        assert!(
            !packages.exists() || fs::read_dir(&packages).unwrap().next().is_none(),
            "failed route setup must remove its prepared Return package"
        );
        assert!(
            snapshot.exists(),
            "a failed Return keeps its exact durable snapshot"
        );
        assert_eq!(
            runtime
                .native_agents
                .lock()
                .movement_status("movement-no-route")
                .unwrap()
                .code
                .as_deref(),
            Some("result_return_retry_required")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconciliation_rejects_a_replaced_or_wrong_selected_host() {
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("task-reconcile", "host:current", "/workspace", "task")
            .unwrap();
        let fact = NativeAgentReconciliationV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-reconcile".into(),
            movement_id: None,
            executing_host_ref: "host:replaced".into(),
            task_state: NativeAgentTaskStateV1::Completed,
            movement_state: None,
            result_digest: None,
            apply_completed: false,
            code: None,
            task_output: None,
        };
        assert!(service.record_remote_reconciliation(fact).is_err());
    }

    #[test]
    fn restart_surfaces_only_safe_unresolved_bridge_projection() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-projection-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .queue_bridge_remote_task(
                "room-recovery",
                "task-recovery",
                "host:remote",
                "/private/physical/secret-workspace",
                "private task instructions",
            )
            .unwrap();
        let after = NativeAgentServiceV1::with_paths(paths).unwrap();
        let projection = after
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .expect("unresolved task must be recoverable");
        assert_eq!(projection.task.state, NativeAgentTaskStateV1::Interrupted);
        assert_eq!(
            projection.task.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        let encoded = serde_json::to_string(&projection).unwrap();
        for private in [
            "/private/physical",
            "private task instructions",
            "threadId",
            "turnId",
            "provider",
            "auth",
            "process",
            "sessionReused",
        ] {
            assert!(
                !encoded.contains(private),
                "leaked private field: {private}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_projection_drains_every_unresolved_bridge_task() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-recovery-drain-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        for task_id in ["task-a", "task-b"] {
            before
                .queue_bridge_remote_task(
                    "room-recovery",
                    task_id,
                    "host:remote",
                    &format!("/private/{task_id}"),
                    "private task instructions",
                )
                .unwrap();
        }
        before
            .queue_bridge_remote_task(
                "room-recovery",
                "task-terminal-history",
                "host:remote",
                "/private/history",
                "already terminal",
            )
            .unwrap();
        before
            .cancel_remote_task("task-terminal-history", "host:remote")
            .unwrap();

        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        let first = after
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .expect("first unresolved task must be reachable");
        assert_eq!(first.task.task_id, "task-a");
        after
            .stop_bridge_task_authority("room-recovery", &first.task.task_id)
            .unwrap();

        let second = after
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .expect("next unresolved task must become reachable");
        assert_eq!(second.task.task_id, "task-b");
        assert_ne!(second.task.task_id, "task-terminal-history");
        after
            .stop_bridge_task_authority("room-recovery", &second.task.task_id)
            .unwrap();

        assert!(after
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn consequence_recovery_abandonment_preserves_completed_task_and_drains_next_item() {
        for (index, code) in [
            "result_apply_interrupted",
            "conflict_result_retention_required",
            "native_agent_reconciliation_required",
        ]
        .into_iter()
        .enumerate()
        {
            let (root, source, _) = fixture();
            fs::write(source.join("baseline.txt"), b"baseline").unwrap();
            let paths = durable_paths(&root);
            let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
            let task_id = format!("task-a-{index}");
            let movement_id = format!("movement-a-{index}");
            service
                .propose_bridge_workspace_movement(
                    "room-recovery",
                    &movement_id,
                    &task_id,
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
            {
                let mut tasks = service.tasks.lock().unwrap();
                let task = tasks.get_mut(&task_id).unwrap();
                task.state = NativeAgentTaskStateV1::Completed;
                task.code = Some("native_agent_completed".into());
                drop(tasks);
                let movement = service.workspace_movements.get_mut(&movement_id).unwrap();
                movement.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
                movement.status.code = Some(code.into());
            }
            service.persist_envelope(&task_id, None).unwrap();
            let next_task_id = format!("task-b-{index}");
            service
                .queue_bridge_remote_task(
                    "room-recovery",
                    &next_task_id,
                    "host:remote",
                    "/remote/workspace",
                    "next task",
                )
                .unwrap();
            {
                let mut tasks = service.tasks.lock().unwrap();
                let task = tasks.get_mut(&next_task_id).unwrap();
                task.state = NativeAgentTaskStateV1::Interrupted;
                task.code = Some("native_agent_reconciliation_required".into());
            }
            service.persist_envelope(&next_task_id, None).unwrap();

            let first = service
                .recovery_projection_for_bridge("room-recovery")
                .unwrap()
                .expect("consequence movement must be projected");
            assert_eq!(first.task.task_id, task_id);
            assert_eq!(first.task.state, NativeAgentTaskStateV1::Completed);
            assert_eq!(first.movement.as_ref().unwrap().code.as_deref(), Some(code));

            let stopped = service
                .stop_bridge_task_authority("room-recovery", &task_id)
                .unwrap();
            assert_eq!(stopped.state, NativeAgentTaskStateV1::Completed);
            assert_eq!(
                service.movement_status(&movement_id).unwrap().state,
                NativeAgentWorkspaceMovementStateV1::Cancelled
            );
            assert_eq!(
                service
                    .movement_status(&movement_id)
                    .unwrap()
                    .code
                    .as_deref(),
                Some("native_agent_recovery_abandoned")
            );

            // An authoritative late execution fact cannot restore an abandoned
            // Pastey movement or apply its result.
            service
                .record_bridge_remote_reconciliation(
                    "room-recovery",
                    NativeAgentReconciliationV1 {
                        schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                        task_id: task_id.clone(),
                        movement_id: Some(movement_id.clone()),
                        executing_host_ref: "host:remote".into(),
                        task_state: NativeAgentTaskStateV1::Completed,
                        movement_state: Some(NativeAgentWorkspaceMovementStateV1::ReturningResult),
                        result_digest: None,
                        apply_completed: false,
                        code: None,
                        task_output: None,
                    },
                )
                .unwrap();
            assert_eq!(
                service.movement_status(&movement_id).unwrap().state,
                NativeAgentWorkspaceMovementStateV1::Cancelled
            );
            drop(service);

            let mut recovered = NativeAgentServiceV1::with_paths(paths).unwrap();
            let next = recovered
                .recovery_projection_for_bridge("room-recovery")
                .unwrap()
                .expect("next unresolved recovery item must become reachable after restart");
            assert_eq!(next.task.task_id, next_task_id);
            assert!(next.task.task_id != task_id);
            recovered
                .stop_bridge_task_authority("room-recovery", &next_task_id)
                .unwrap();
            assert!(recovered
                .recovery_projection_for_bridge("room-recovery")
                .unwrap()
                .is_none());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn outcome_unknown_reconciliation_reveals_next_bridge_recovery_item() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-reconcile-drain-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths).unwrap();
        for (task_id, code) in [
            ("task-a", "native_agent_outcome_unknown"),
            ("task-b", "native_agent_reconciliation_required"),
        ] {
            service
                .queue_bridge_remote_task(
                    "room-recovery",
                    task_id,
                    "host:remote",
                    "/remote/workspace",
                    "task",
                )
                .unwrap();
            {
                let mut tasks = service.tasks.lock().unwrap();
                let task = tasks.get_mut(task_id).unwrap();
                task.state = NativeAgentTaskStateV1::Interrupted;
                task.code = Some(code.into());
            }
            service.persist_envelope(task_id, None).unwrap();
        }
        assert_eq!(
            service
                .recovery_projection_for_bridge("room-recovery")
                .unwrap()
                .unwrap()
                .task
                .task_id,
            "task-a"
        );
        service
            .record_bridge_remote_reconciliation(
                "room-recovery",
                NativeAgentReconciliationV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-a".into(),
                    movement_id: None,
                    executing_host_ref: "host:remote".into(),
                    task_state: NativeAgentTaskStateV1::Completed,
                    movement_state: None,
                    result_digest: None,
                    apply_completed: false,
                    code: None,
                    task_output: None,
                },
            )
            .unwrap();
        let next = service
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .expect("resolved unknown outcome must drain to the next item");
        assert_eq!(next.task.task_id, "task-b");
        service
            .stop_bridge_task_authority("room-recovery", "task-b")
            .unwrap();
        assert!(service
            .recovery_projection_for_bridge("room-recovery")
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(root);
    }

    fn prepare_from(
        movement_id: &str,
        task_id: &str,
        source_host_ref: &str,
    ) -> NativeAgentWorkspacePrepareV1 {
        NativeAgentWorkspacePrepareV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: movement_id.into(),
            task_id: task_id.into(),
            source_host_ref: source_host_ref.into(),
            target_host_ref: "host:local".into(),
            agent_capability: CODEX_CAPABILITY_ID.into(),
            task: "edit".into(),
            resume: true,
            source_object: movement_object(),
            source_digest: "a".repeat(64),
            source_bytes: 0,
        }
    }

    #[test]
    fn workspace_prepare_is_bound_to_the_authenticated_source_host() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-prepare-source-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        let owned_by = |bridge_id: &str| Some(TaskOwnerV1::Bridge(bridge_id.into()));

        // An authenticated peer claiming another source Host prepares nothing.
        assert!(service
            .accept_bridge_workspace_prepare_from(
                "room-a",
                "host:source",
                prepare_from("movement-forged", "task-forged", "host:other"),
            )
            .is_err());
        assert!(service.movement_status("movement-forged").is_err());
        assert_eq!(service.task_owner("task-forged").unwrap(), None);
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-forged")
                .unwrap()
                .is_none()
        );

        let request = prepare_from("movement-bound", "task-bound", "host:source");
        service
            .accept_bridge_workspace_prepare_from("room-a", "host:source", request.clone())
            .unwrap();
        assert_eq!(
            service.task_owner("task-bound").unwrap(),
            owned_by("room-a")
        );
        // Another Bridge cannot adopt it, even from the same source Host, and
        // a new movement cannot reuse its task identity.
        assert!(service
            .accept_bridge_workspace_prepare_from("room-b", "host:source", request.clone())
            .is_err());
        assert!(service
            .accept_bridge_workspace_prepare_from(
                "room-a",
                "host:source",
                prepare_from("movement-reused", "task-bound", "host:source"),
            )
            .is_err());
        assert!(service.movement_status("movement-reused").is_err());
        // The owning Bridge's exact replay stays idempotent.
        service
            .accept_bridge_workspace_prepare_from("room-a", "host:source", request)
            .unwrap();
        assert_eq!(
            service.task_owner("task-bound").unwrap(),
            owned_by("room-a")
        );
        assert!(service.cancel_bridge_task("room-b", "task-bound").is_err());

        drop(service);
        let restarted = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(
            restarted.task_owner("task-bound").unwrap(),
            owned_by("room-a")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approval_never_moves_a_movement_into_another_authority() {
        let (root, source, _) = fixture();
        fs::write(source.join("before.txt"), b"baseline").unwrap();
        let mut service = NativeAgentServiceV1::default();
        let propose = |service: &mut NativeAgentServiceV1,
                       bridge_id: Option<&str>,
                       movement_id: &str,
                       task_id: &str| match bridge_id {
            Some(bridge_id) => service.propose_bridge_workspace_movement(
                bridge_id,
                movement_id,
                task_id,
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            ),
            None => service.propose_workspace_movement(
                movement_id,
                task_id,
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            ),
        };

        // An unbound proposal is Local: no Bridge can approve or adopt it.
        propose(&mut service, None, "movement-local", "task-local").unwrap();
        assert!(service
            .approve_workspace_movement("movement-local", "room-a", "host:source", &root)
            .is_err());
        assert!(propose(&mut service, Some("room-a"), "movement-other", "task-local").is_err());
        assert_eq!(
            service.task_owner("task-local").unwrap(),
            Some(TaskOwnerV1::Local)
        );

        // A Bridge's proposal is approved only over that Bridge.
        propose(&mut service, Some("room-a"), "movement-a", "task-a").unwrap();
        assert!(propose(&mut service, Some("room-b"), "movement-b", "task-a").is_err());
        assert!(service
            .approve_workspace_movement("movement-a", "room-b", "host:source", &root)
            .is_err());
        assert_eq!(
            service.task_owner("task-a").unwrap(),
            Some(TaskOwnerV1::Bridge("room-a".into()))
        );
        let (_, _, package) = service
            .approve_workspace_movement("movement-a", "room-a", "host:source", &root)
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepared_workspace_cancel_is_idempotent_and_blocks_late_landing() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-prepare-cancel-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                "room",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-prepared".into(),
                    task_id: "task-prepared".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "a".repeat(64),
                    source_bytes: 0,
                },
            )
            .unwrap();

        // The requester revokes prepared authority over its own Bridge.
        let first = service.cancel_bridge_task("room", "task-prepared").unwrap();
        let duplicate = service.cancel_bridge_task("room", "task-prepared").unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(first.state, NativeAgentTaskStateV1::Cancelled);
        assert_eq!(
            service.movement_status("movement-prepared").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );

        let metadata = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-prepared".into(),
            task_id: "task-prepared".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Outbound,
            bridge_id: "room".into(),
            source_host_ref: "host:source".into(),
            destination_host_ref: "host:local".into(),
            object: movement_object(),
            content_digest: "a".repeat(64),
            logical_byte_count: 0,
        };
        assert!(service
            .validate_workspace_transfer(&metadata, "host:local")
            .is_err());
        let landed = root.join("late-landing");
        fs::create_dir_all(&landed).unwrap();
        let late = service
            .start_received_workspace_task_with_executable(
                Path::new("/definitely/missing/codex"),
                "movement-prepared",
                &landed,
            )
            .unwrap();
        assert_eq!(late.state, NativeAgentTaskStateV1::Cancelled);
        assert!(service.active_exclusivity.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_bridge_stop_cancels_recovered_target_side_authority_without_a_route() {
        let root = std::env::temp_dir().join(format!("pastey-native-stop-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .accept_bridge_workspace_prepare_from(
                "room-recovery",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-target".into(),
                    task_id: "task-target".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:local".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "a".repeat(64),
                    source_bytes: 0,
                },
            )
            .unwrap();
        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert!(after
            .bridge_remote_target("room-recovery", "task-target")
            .unwrap()
            .is_none());
        assert_eq!(
            after
                .stop_bridge_task_authority("room-recovery", "task-target")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert_eq!(
            after.movement_status("movement-target").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_terminal_and_apply_states_are_monotonic() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("task-monotonic", "host:remote", "/workspace", "task")
            .unwrap();
        let status = |state, code: Option<&str>| NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-monotonic".into(),
            executing_host_ref: "host:remote".into(),
            status: NativeAgentTaskStatusV1 {
                schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: "task-monotonic".into(),
                agent_id: CODEX_CAPABILITY_ID.into(),
                workspace_name: "workspace".into(),
                session_reused: false,
                state,
                result: None,
                code: code.map(str::to_owned),
                output: None,
                conversation: None,
                native_decisions: Vec::new(),
            },
        };
        service
            .record_remote_status(status(NativeAgentTaskStateV1::Completed, Some("done")))
            .unwrap();
        assert_eq!(
            service
                .record_remote_status(status(NativeAgentTaskStateV1::Running, Some("late")))
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Completed
        );
        service
            .record_remote_reconciliation(NativeAgentReconciliationV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "task-monotonic".into(),
                movement_id: None,
                executing_host_ref: "host:remote".into(),
                task_state: NativeAgentTaskStateV1::Running,
                movement_state: None,
                result_digest: None,
                apply_completed: false,
                code: Some("stale".into()),
                task_output: None,
            })
            .unwrap();
        assert_eq!(
            service.task_status("task-monotonic").unwrap().state,
            NativeAgentTaskStateV1::Completed
        );

        service
            .propose_workspace_movement(
                "movement-apply-monotonic",
                "task-apply-monotonic",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        let movement = service
            .workspace_movements
            .get_mut("movement-apply-monotonic")
            .unwrap();
        movement.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
        movement.apply_completed = true;
        service
            .record_remote_status(NativeAgentStatusV1 {
                schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                task_id: "task-apply-monotonic".into(),
                executing_host_ref: "host:remote".into(),
                status: NativeAgentTaskStatusV1 {
                    schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                    task_id: "task-apply-monotonic".into(),
                    agent_id: CODEX_CAPABILITY_ID.into(),
                    workspace_name: "workspace".into(),
                    session_reused: false,
                    state: NativeAgentTaskStateV1::Running,
                    result: None,
                    code: Some("late".into()),
                    output: None,
                    conversation: None,
                    native_decisions: Vec::new(),
                },
            })
            .unwrap();
        assert_eq!(
            service
                .movement_status("movement-apply-monotonic")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );

        let mut cancelled = NativeAgentServiceV1::default();
        cancelled
            .queue_remote_task("task-cancelled", "host:remote", "/workspace", "task")
            .unwrap();
        cancelled
            .cancel_remote_task("task-cancelled", "host:remote")
            .unwrap();
        let mut late = status(NativeAgentTaskStateV1::Completed, Some("late-complete"));
        late.task_id = "task-cancelled".into();
        late.status.task_id = "task-cancelled".into();
        assert_eq!(
            cancelled.record_remote_status(late).unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ambiguous_outbound_receipt_requires_reconciliation_without_rerun() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        for (movement, task) in [
            ("movement-ambiguous", "task-ambiguous"),
            ("movement-next", "task-next"),
        ] {
            service
                .propose_bridge_workspace_movement(
                    "room",
                    movement,
                    task,
                    &source,
                    "host:remote",
                    movement_object(),
                    "edit",
                    true,
                )
                .unwrap();
        }
        let approved = service
            .approve_workspace_movement(
                "movement-ambiguous",
                "room",
                "host:source",
                &paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&approved.2);
        let interrupted = service
            .mark_outbound_workspace_delivery_failed("movement-ambiguous", true)
            .unwrap();
        assert_eq!(
            interrupted.state,
            NativeAgentWorkspaceMovementStateV1::Interrupted
        );
        assert_eq!(
            interrupted.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        service
            .record_bridge_remote_reconciliation(
                "room",
                NativeAgentReconciliationV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-ambiguous".into(),
                    movement_id: Some("movement-ambiguous".into()),
                    executing_host_ref: "host:remote".into(),
                    task_state: NativeAgentTaskStateV1::Queued,
                    movement_state: Some(NativeAgentWorkspaceMovementStateV1::TransferringToAgent),
                    result_digest: None,
                    apply_completed: false,
                    code: Some("workspace_transfer_pending".into()),
                    task_output: None,
                },
            )
            .unwrap();
        assert_eq!(
            service
                .task_status("task-ambiguous")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_reconciliation_required")
        );
        assert_eq!(
            service
                .movement_status("movement-ambiguous")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_reconciliation_required")
        );
        assert_eq!(
            service.task_status("task-ambiguous").unwrap().state,
            NativeAgentTaskStateV1::Interrupted
        );
        assert!(!service.task_workspaces.contains_key("task-ambiguous"));
        assert!(service
            .approve_workspace_movement("movement-next", "room", "host:source", &paths.temp_dir,)
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn successful_remote_finish_with_failed_sender_status_write_keeps_source_owned() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, source, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; echo run >> \"$PWD/turn-runs\"; echo '{terminal}'"
        ));
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let received = root.join("received");
        fs::create_dir(&received).unwrap();
        fs::write(received.join("baseline.txt"), b"baseline").unwrap();
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-finish",
                "movement-finish",
                "task-finish",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-finish",
                "movement-next",
                "task-next",
                &source,
                "host:remote",
                movement_object(),
                "edit again",
                true,
            )
            .unwrap();
        let (prepare, _, package) = requester
            .approve_workspace_movement(
                "movement-finish",
                "room-finish",
                "host:source",
                &requester_paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let mut executor = NativeAgentServiceV1::with_paths(executor_paths).unwrap();
        executor
            .accept_bridge_workspace_prepare_from(
                "room-finish",
                &prepare.source_host_ref.clone(),
                prepare,
            )
            .unwrap();
        // Receiver /finish has completed landing and started the one Agent
        // turn before this sender-local UI write fails.
        land_and_accept(&mut executor, &agent, "movement-finish", &received).unwrap();
        let receipt_ambiguous = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let write = crate::transfer::record_successful_finish_before_sender_bookkeeping(
            Some(&receipt_ambiguous),
            || invalid("injected sender room-item status failure"),
        );
        assert!(write.is_err());
        assert!(receipt_ambiguous.load(std::sync::atomic::Ordering::SeqCst));
        let interrupted = requester
            .mark_outbound_workspace_delivery_failed(
                "movement-finish",
                receipt_ambiguous.load(std::sync::atomic::Ordering::SeqCst),
            )
            .unwrap();
        assert_eq!(
            interrupted.code.as_deref(),
            Some("native_agent_reconciliation_required")
        );
        assert!(movement_holds_source_ownership(
            &requester
                .workspace_movements
                .get("movement-finish")
                .unwrap()
                .status
        ));
        assert!(requester
            .approve_workspace_movement(
                "movement-next",
                "room-finish",
                "host:source",
                &requester_paths.temp_dir
            )
            .is_err());
        assert!(requester
            .start_codex_task_with_executable(&agent, &source, "must not start")
            .is_err());
        assert_eq!(
            wait_for_terminal(&executor, "task-finish").state,
            NativeAgentTaskStateV1::Completed
        );
        executor
            .captured_result_snapshot_for_return("movement-finish")
            .unwrap();
        executor.mark_result_return_pending("movement-finish");
        let fact = executor
            .reconciliation_fact(
                "room-finish",
                "task-finish",
                Some("movement-finish"),
                "host:remote",
                "host:source",
            )
            .unwrap();
        requester
            .record_bridge_remote_reconciliation("room-finish", fact)
            .unwrap();
        let result = crate::safe_file_identity::capture_regular_file_set_identity(
            &received,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        let transfer = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-finish".into(),
            task_id: "task-finish".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room-finish".into(),
            source_host_ref: "host:remote".into(),
            destination_host_ref: "host:source".into(),
            object: movement_object(),
            content_digest: result.digest,
            logical_byte_count: result.byte_count,
        };
        requester
            .validate_workspace_transfer(&transfer, "host:source")
            .unwrap();
        assert_eq!(
            requester
                .apply_received_workspace_return("movement-finish", &received)
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        assert_eq!(
            fs::read_to_string(received.join("turn-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn durable_approval_failure_restores_review_and_cleans_package() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .propose_bridge_workspace_movement(
                "room",
                "movement-approval-failure",
                "task-approval-failure",
                &source,
                "host:remote",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        fs::remove_file(&paths.db_path).unwrap();
        assert!(service
            .approve_workspace_movement(
                "movement-approval-failure",
                "room",
                "host:source",
                &paths.temp_dir,
            )
            .is_err());
        assert_eq!(
            service
                .movement_status("movement-approval-failure")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::AwaitingApproval
        );
        let package_root = paths.temp_dir.join("native-v2-transfer-packages");
        assert!(
            !package_root.exists() || fs::read_dir(package_root).unwrap().next().is_none(),
            "approval rollback must not retain a generated transfer package"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bridge_burn_purges_only_bound_native_authority_and_blocks_late_facts() {
        let root = std::env::temp_dir().join(format!("pastey-native-burn-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let source = root.join("source");
        let returned = root.join("returned");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&returned).unwrap();
        fs::write(source.join("note.txt"), b"original").unwrap();
        fs::write(returned.join("note.txt"), b"late return").unwrap();
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .queue_bridge_remote_task(
                "room-burn",
                "task-bound",
                "host:remote",
                "/remote/workspace",
                "task",
            )
            .unwrap();
        service
            .queue_remote_task("task-unrelated", "host:other", "/other/workspace", "task")
            .unwrap();
        service
            .propose_bridge_workspace_movement(
                "room-burn",
                "movement-bound",
                "task-return-bound",
                &source,
                "host:remote",
                movement_object(),
                "task",
                true,
            )
            .unwrap();
        service.purge_bridge_authority("room-burn").unwrap();
        assert!(service.task_status("task-bound").is_err());
        assert!(service.task_status("task-unrelated").is_ok());
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-bound")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-unrelated")
                .unwrap()
                .is_some()
        );
        let late = NativeAgentStatusV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-bound".into(),
            executing_host_ref: "host:remote".into(),
            status: NativeAgentTaskStatusV1 {
                schema_version: NATIVE_AGENT_TASK_SCHEMA.into(),
                task_id: "task-bound".into(),
                agent_id: CODEX_CAPABILITY_ID.into(),
                workspace_name: "workspace".into(),
                session_reused: false,
                state: NativeAgentTaskStateV1::Completed,
                result: Some("late".into()),
                code: None,
                output: None,
                conversation: None,
                native_decisions: Vec::new(),
            },
        };
        assert!(service
            .record_bridge_remote_status("room-burn", late)
            .is_err());
        assert!(service
            .record_bridge_remote_reconciliation(
                "room-burn",
                NativeAgentReconciliationV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-bound".into(),
                    movement_id: None,
                    executing_host_ref: "host:remote".into(),
                    task_state: NativeAgentTaskStateV1::Completed,
                    movement_state: None,
                    result_digest: None,
                    apply_completed: false,
                    code: None,
                    task_output: None,
                },
            )
            .is_err());
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-bound")
                .unwrap()
                .is_none()
        );
        assert!(service
            .apply_received_workspace_return("movement-bound", &returned)
            .is_err());
        assert_eq!(fs::read(source.join("note.txt")).unwrap(), b"original");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn late_local_observer_after_bridge_burn_cannot_recreate_envelope() {
        let terminal = r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#;
        let (root, workspace, agent) = fixture_with_turn_start_behavior(&format!(
            "echo '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"native-turn\"}}}}}}'; sleep 1; echo '{terminal}'"
        ));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        start_accepted_bridge_codex_task(
            &mut service,
            "room-burn",
            &agent,
            "task-burn-observer",
            &workspace,
            "wait",
        )
        .unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();
        let controller = service
            .codex
            .wait_for_session_controller(&canonical_workspace);
        for _ in 0..300 {
            if controller.active_turn.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.active_turn.lock().unwrap().is_some());
        service.purge_bridge_authority("room-burn").unwrap();
        assert!(service.revoked_bridges.contains("room-burn"));
        assert!(!service.codex.has_session(&canonical_workspace));
        assert!(controller.interrupt_requested.load(Ordering::Acquire));
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-burn-observer")
                .unwrap()
                .is_none()
        );
        thread::sleep(Duration::from_millis(1100));
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-burn-observer")
                .unwrap()
                .is_none()
        );
        assert!(service.task_status("task-burn-observer").is_err());
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bridge_burn_removes_retained_conflict_and_app_owned_result() {
        let (root, paths, mut service, conflict) =
            durable_conflict_fixture("movement-burn", "task-burn");
        service
            .task_bridges
            .insert("task-burn".into(), "room-burn".into());
        service
            .workspace_movements
            .get_mut("movement-burn")
            .unwrap()
            .bridge_id = Some("room-burn".into());
        service.persist_envelope("task-burn", None).unwrap();
        assert!(conflict.retained_tree.exists());
        service.purge_bridge_authority("room-burn").unwrap();
        assert!(!conflict.retained_tree.exists());
        assert!(
            crate::storage::get_native_agent_conflict(&paths, "movement-burn")
                .unwrap()
                .is_none()
        );
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-burn")
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn requester_reconciles_one_completed_exact_return_after_session_loss() {
        let (root, source, agent) = fixture_with_turn_start_behavior(
            r#"echo run >> "$PWD/agent-runs"; echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'; echo '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'"#,
        );
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-exact",
                "movement-exact",
                "task-exact",
                &source,
                "host:executor",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        let (prepare, _, package) = requester
            .approve_workspace_movement(
                "movement-exact",
                "room-exact",
                "host:source",
                &requester_paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let task_workspace = crate::safe_file_identity::create_private_tree_root(
            &executor_paths.temp_dir,
            "native-v2-file-sets",
            "exact-return",
        )
        .unwrap();
        fs::write(task_workspace.join("baseline.txt"), b"baseline").unwrap();
        let mut executor = NativeAgentServiceV1::with_paths(executor_paths.clone()).unwrap();
        executor
            .accept_bridge_workspace_prepare_from(
                "room-exact",
                &prepare.source_host_ref.clone(),
                prepare,
            )
            .unwrap();
        land_and_accept(&mut executor, &agent, "movement-exact", &task_workspace).unwrap();
        assert_eq!(
            wait_for_terminal(&executor, "task-exact").state,
            NativeAgentTaskStateV1::Completed
        );
        let (_, snapshot, _, identity) = executor
            .captured_result_snapshot_for_return("movement-exact")
            .unwrap();
        let retry = NativeAgentRetryResultReturnV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            retry_id: "fresh-exact-retry".into(),
            movement_id: "movement-exact".into(),
            task_id: "task-exact".into(),
            target_host_ref: "host:executor".into(),
        };
        executor
            .authorize_result_return_retry("room-exact", &retry, "host:source", "host:executor")
            .unwrap();
        assert!(executor
            .reconciliation_fact(
                "room-exact",
                "task-exact",
                Some("movement-exact"),
                "host:executor",
                "host:other",
            )
            .is_err());
        requester.revoke_bridge_session("room-exact").unwrap();
        requester
            .record_bridge_remote_status(
                "room-exact",
                NativeAgentStatusV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-exact".into(),
                    executing_host_ref: "host:executor".into(),
                    status: executor.task_status("task-exact").unwrap(),
                },
            )
            .unwrap();
        assert_eq!(
            requester.movement_status("movement-exact").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Interrupted
        );
        let fact = executor
            .reconciliation_fact(
                "room-exact",
                "task-exact",
                Some("movement-exact"),
                "host:executor",
                "host:source",
            )
            .unwrap();
        assert_eq!(
            fact.result_digest.as_deref(),
            Some(identity.digest.as_str())
        );
        executor.mark_result_return_pending("movement-exact");
        let mut wrong_host = fact.clone();
        wrong_host.executing_host_ref = "host:other".into();
        assert!(requester
            .record_bridge_remote_reconciliation("room-exact", wrong_host)
            .is_err());
        let mut wrong_movement = fact.clone();
        wrong_movement.movement_id = Some("movement-other".into());
        assert!(requester
            .record_bridge_remote_reconciliation("room-exact", wrong_movement)
            .is_err());
        let mut nonterminal = fact.clone();
        nonterminal.task_state = NativeAgentTaskStateV1::Running;
        requester
            .record_bridge_remote_reconciliation("room-exact", nonterminal)
            .unwrap();
        assert_eq!(
            requester.movement_status("movement-exact").unwrap().state,
            NativeAgentWorkspaceMovementStateV1::Interrupted
        );
        requester
            .record_bridge_remote_reconciliation("room-exact", fact.clone())
            .unwrap();
        assert_eq!(
            requester
                .movement_status("movement-exact")
                .unwrap()
                .code
                .as_deref(),
            Some("result_return_retry_required")
        );
        assert!(requester
            .workspace_movements
            .get("movement-exact")
            .unwrap()
            .result_identity
            .is_none());
        requester.revoke_bridge_session("room-exact").unwrap();
        drop(requester);
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        assert_eq!(
            requester
                .movement_status("movement-exact")
                .unwrap()
                .code
                .as_deref(),
            Some("result_return_retry_required")
        );
        requester
            .authorize_source_pending_return_retry("room-exact", "movement-exact")
            .unwrap();
        let mut changed_digest = fact;
        changed_digest.result_digest = Some("0".repeat(64));
        assert!(requester
            .record_bridge_remote_reconciliation("room-exact", changed_digest)
            .is_err());
        let mut transfer = NativeAgentWorkspaceTransferV1 {
            schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
            movement_id: "movement-exact".into(),
            task_id: "task-exact".into(),
            phase: NativeAgentWorkspaceTransferPhaseV1::Return,
            bridge_id: "room-exact".into(),
            source_host_ref: "host:executor".into(),
            destination_host_ref: "host:source".into(),
            object: movement_object(),
            content_digest: identity.digest.clone(),
            logical_byte_count: identity.byte_count,
        };
        transfer.content_digest = "0".repeat(64);
        assert!(requester
            .validate_workspace_transfer(&transfer, "host:source")
            .is_err());
        transfer.content_digest = identity.digest.clone();
        requester
            .validate_workspace_transfer(&transfer, "host:source")
            .unwrap();
        assert_eq!(
            requester
                .apply_received_workspace_return("movement-exact", &snapshot)
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Completed
        );
        fs::write(source.join("later.txt"), b"user edit").unwrap();
        requester
            .apply_received_workspace_return("movement-exact", &snapshot)
            .unwrap();
        assert_eq!(fs::read(source.join("later.txt")).unwrap(), b"user edit");
        assert_eq!(
            fs::read_to_string(task_workspace.join("agent-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unrepresentable_completed_result_reaches_requester_for_explicit_abandonment() {
        let (root, source, agent) = fixture_with_turn_start_behavior(
            r#"echo run >> "$PWD/agent-runs"; touch "$PWD/result.sh"; chmod +x "$PWD/result.sh"; echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'; echo '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'"#,
        );
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let requester_paths = durable_paths(&root.join("requester"));
        let executor_paths = durable_paths(&root.join("executor"));
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-unrepresentable",
                "movement-unrepresentable",
                "task-unrepresentable",
                &source,
                "host:executor",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        let (prepare, _, package) = requester
            .approve_workspace_movement(
                "movement-unrepresentable",
                "room-unrepresentable",
                "host:source",
                &requester_paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        let task_workspace = crate::safe_file_identity::create_private_tree_root(
            &executor_paths.temp_dir,
            "native-v2-file-sets",
            "unrepresentable",
        )
        .unwrap();
        fs::write(task_workspace.join("baseline.txt"), b"baseline").unwrap();
        let mut executor = NativeAgentServiceV1::with_paths(executor_paths).unwrap();
        executor
            .accept_bridge_workspace_prepare_from(
                "room-unrepresentable",
                &prepare.source_host_ref.clone(),
                prepare,
            )
            .unwrap();
        land_and_accept(
            &mut executor,
            &agent,
            "movement-unrepresentable",
            &task_workspace,
        )
        .unwrap();
        assert_eq!(
            wait_for_terminal(&executor, "task-unrepresentable").state,
            NativeAgentTaskStateV1::Completed
        );
        assert!(executor
            .captured_result_snapshot_for_return("movement-unrepresentable")
            .is_err());
        executor.mark_result_return_pending("movement-unrepresentable");
        assert_eq!(
            executor
                .movement_status("movement-unrepresentable")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_result_snapshot_recovery_failed")
        );
        requester
            .record_bridge_remote_status(
                "room-unrepresentable",
                NativeAgentStatusV1 {
                    schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
                    task_id: "task-unrepresentable".into(),
                    executing_host_ref: "host:executor".into(),
                    status: executor.task_status("task-unrepresentable").unwrap(),
                },
            )
            .unwrap();
        assert_eq!(
            requester
                .movement_status("movement-unrepresentable")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::ReturningResult
        );
        let fact = executor
            .reconciliation_fact(
                "room-unrepresentable",
                "task-unrepresentable",
                Some("movement-unrepresentable"),
                "host:executor",
                "host:source",
            )
            .unwrap();
        requester
            .record_bridge_remote_reconciliation("room-unrepresentable", fact)
            .unwrap();
        let failed = requester
            .movement_status("movement-unrepresentable")
            .unwrap();
        assert_eq!(
            failed.state,
            NativeAgentWorkspaceMovementStateV1::Interrupted
        );
        assert_eq!(
            failed.code.as_deref(),
            Some("native_agent_result_snapshot_recovery_failed")
        );
        drop(requester);
        let mut requester = NativeAgentServiceV1::with_paths(requester_paths.clone()).unwrap();
        assert_eq!(
            requester
                .movement_status("movement-unrepresentable")
                .unwrap()
                .code
                .as_deref(),
            Some("native_agent_result_snapshot_recovery_failed")
        );
        assert!(requester
            .recovery_projection_for_bridge("room-unrepresentable")
            .unwrap()
            .is_some());
        assert!(requester
            .propose_bridge_workspace_movement(
                "room-unrepresentable",
                "movement-next",
                "task-next",
                &source,
                "host:executor",
                movement_object(),
                "next",
                true,
            )
            .is_ok());
        assert!(requester
            .approve_workspace_movement(
                "movement-next",
                "room-unrepresentable",
                "host:source",
                &requester_paths.temp_dir,
            )
            .is_err());
        assert_eq!(
            requester
                .stop_bridge_task_authority("room-unrepresentable", "task-unrepresentable")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(
            requester
                .movement_status("movement-unrepresentable")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        let (_, _, package) = requester
            .approve_workspace_movement(
                "movement-next",
                "room-unrepresentable",
                "host:source",
                &requester_paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        assert_eq!(
            fs::read_to_string(task_workspace.join("agent-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(
            executor.task_status("task-unrepresentable").unwrap().state,
            NativeAgentTaskStateV1::Completed
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn post_start_persistence_failure_keeps_running_workspace_and_single_turn() {
        let (root, _, agent) = fixture_with_turn_start_behavior(
            r#"echo run >> "$PWD/agent-runs"; echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'; sleep 0.1; echo '{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}'"#,
        );
        let paths = durable_paths(&root);
        let workspace = crate::safe_file_identity::create_private_tree_root(
            &paths.temp_dir,
            "native-v2-file-sets",
            "post-start-persist",
        )
        .unwrap();
        fs::write(workspace.join("baseline.txt"), b"baseline").unwrap();
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .accept_bridge_workspace_prepare_from(
                "room-persist",
                "host:source",
                NativeAgentWorkspacePrepareV1 {
                    schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
                    movement_id: "movement-persist".into(),
                    task_id: "task-persist".into(),
                    source_host_ref: "host:source".into(),
                    target_host_ref: "host:executor".into(),
                    agent_capability: CODEX_CAPABILITY_ID.into(),
                    task: "edit".into(),
                    resume: true,
                    source_object: movement_object(),
                    source_digest: "b".repeat(64),
                    source_bytes: 1,
                },
            )
            .unwrap();
        service.fail_post_start_persist_once = true;
        land_and_accept(&mut service, &agent, "movement-persist", &workspace).unwrap();
        let stored = crate::storage::get_native_agent_envelope(&paths, "task-persist")
            .unwrap()
            .unwrap();
        let persisted: PersistedNativeAgentEnvelopeV1 =
            serde_json::from_str(&stored.record_json).unwrap();
        assert_eq!(
            persisted.movement.unwrap().task_workspace.as_deref(),
            Some(workspace.canonicalize().unwrap().as_path())
        );
        assert!(workspace.exists());
        assert_eq!(
            wait_for_terminal(&service, "task-persist").state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(
            fs::read_to_string(workspace.join("agent-runs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn late_observer_update_cannot_reinsert_deleted_burn_envelope() {
        let (root, workspace, agent) = fixture();
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        service
            .queue_bridge_remote_task(
                "room-burn-observer",
                "task-burn-observer-late",
                "host:remote",
                workspace.to_str().unwrap(),
                "edit",
            )
            .unwrap();
        let completed = NativeAgentTaskStatusV1 {
            state: NativeAgentTaskStateV1::Completed,
            ..service.task_status("task-burn-observer-late").unwrap()
        };
        let stale = serde_json::to_value(&completed).unwrap();
        service
            .purge_bridge_authority("room-burn-observer")
            .unwrap();
        crate::storage::update_native_agent_observed_task_if_present(
            &paths,
            "task-burn-observer-late",
            &stale,
        )
        .unwrap();
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-burn-observer-late")
                .unwrap()
                .is_none()
        );
        let local = service
            .start_codex_task_with_executable(&agent, &workspace, "local work")
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &local.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        for _ in 0..300 {
            let stored = crate::storage::get_native_agent_envelope(&paths, &local.task_id)
                .unwrap()
                .unwrap();
            let persisted: PersistedNativeAgentEnvelopeV1 =
                serde_json::from_str(&stored.record_json).unwrap();
            if persisted.task.state == NativeAgentTaskStateV1::Completed {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let stored = crate::storage::get_native_agent_envelope(&paths, &local.task_id)
            .unwrap()
            .unwrap();
        let persisted: PersistedNativeAgentEnvelopeV1 =
            serde_json::from_str(&stored.record_json).unwrap();
        assert_eq!(persisted.task.state, NativeAgentTaskStateV1::Completed);
        service.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_return_proof_cannot_revive_cancelled_or_burned_requester() {
        let (root, source, _) = fixture();
        fs::write(source.join("baseline.txt"), b"baseline").unwrap();
        let paths = durable_paths(&root);
        let mut requester = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        requester
            .propose_bridge_workspace_movement(
                "room-terminal-proof",
                "movement-terminal-proof",
                "task-terminal-proof",
                &source,
                "host:executor",
                movement_object(),
                "edit",
                true,
            )
            .unwrap();
        let (_, _, package) = requester
            .approve_workspace_movement(
                "movement-terminal-proof",
                "room-terminal-proof",
                "host:source",
                &paths.temp_dir,
            )
            .unwrap();
        crate::regular_file_set_transfer::cleanup_package(&package);
        requester
            .revoke_bridge_session("room-terminal-proof")
            .unwrap();
        requester
            .cancel_remote_task("task-terminal-proof", "host:executor")
            .unwrap();
        let fact = NativeAgentReconciliationV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: "task-terminal-proof".into(),
            movement_id: Some("movement-terminal-proof".into()),
            executing_host_ref: "host:executor".into(),
            task_state: NativeAgentTaskStateV1::Completed,
            movement_state: Some(NativeAgentWorkspaceMovementStateV1::ReturningResult),
            result_digest: Some("a".repeat(64)),
            apply_completed: false,
            code: Some("result_return_retry_required".into()),
            task_output: None,
        };
        requester
            .record_bridge_remote_reconciliation("room-terminal-proof", fact.clone())
            .unwrap();
        assert_eq!(
            requester.task_status("task-terminal-proof").unwrap().state,
            NativeAgentTaskStateV1::Cancelled
        );
        assert_eq!(
            requester
                .movement_status("movement-terminal-proof")
                .unwrap()
                .state,
            NativeAgentWorkspaceMovementStateV1::Cancelled
        );
        requester
            .purge_bridge_authority("room-terminal-proof")
            .unwrap();
        assert!(requester
            .record_bridge_remote_reconciliation("room-terminal-proof", fact)
            .is_err());
        assert!(
            crate::storage::get_native_agent_envelope(&paths, "task-terminal-proof")
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// Two continuations of one Codex thread at almost the same moment, and
    /// a replay of the first: the folder's exclusivity, checked and taken
    /// under the service's one lock, admits exactly one writer; the replay
    /// returns the same task and sends nothing again; nothing is retried.
    #[cfg(unix)]
    #[test]
    fn one_thread_takes_one_continuation_at_a_time_and_a_replay_runs_nothing() {
        let root = std::env::temp_dir().join(format!("pastey-thread-race-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let home = root.join("codex-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(home.join("thread-writer-locks")).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let lock = home.join("thread-writer-locks").join("thread-1.lock");
        let starts = root.join("turn-starts");
        // A Codex whose turn starts and never ends until it is interrupted.
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "codex-cli 0.163.0"; exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1,'*) echo '{{"id":1,"result":{{}}}}' ;;
    *'"id":9,'*) echo '{{"id":9,"result":{{"data":[{{"id":"thread-1","cwd":"{ws}"}}],"nextCursor":null}}}}' ;;
    *'"id":6,'*) touch '{lock}'; echo '{{"id":6,"result":{{"thread":{{"id":"thread-1"}}}}}}' ;;
    *'"id":8,'*) echo '{{"id":8,"result":{{"thread":{{"id":"thread-1","cwd":"{ws}"}}}}}}' ;;
    *'"id":7,'*) echo '{{"id":7,"result":{{"data":[{{"id":"turn-old","status":"completed"}}]}}}}' ;;
    *'"id":3,'*) echo turn >> '{starts}'; echo '{{"id":3,"result":{{"turn":{{"id":"turn-new"}}}}}}' ;;
    *'"id":4,'*) echo '{{"id":4,"result":{{}}}}'
      echo '{{"method":"turn/completed","params":{{"threadId":"thread-1","turn":{{"id":"turn-new","items":[],"status":"interrupted","error":null}}}}}}' ;;
  esac
done
"#,
            ws = workspace.display(),
            lock = lock.display(),
            starts = starts.display(),
        );
        let program = root.join("codex-fixture");
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let mut service = NativeAgentServiceV1::default();
        service.add_codex_thread_adapter_for_test(&program, &home);
        let input = capability::codex::thread_continuation::continue_input(
            &workspace,
            "thread-1",
            "first",
            Some("turn-old"),
        )
        .unwrap();
        let first = service
            .start_local_invocation(CODEX_THREAD_CAPABILITY, "native-agent:first", &input)
            .unwrap();
        let turn_started = || {
            fs::read_to_string(&starts)
                .map(|t| t.lines().count())
                .unwrap_or(0)
        };
        for _ in 0..300 {
            if turn_started() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(turn_started(), 1, "the first continuation runs");
        // A second, independent continuation of the same thread is refused
        // before any adapter sees it: one writer.
        let second =
            service.start_codex_thread_task(&workspace, "thread-1", "second", Some("turn-old"));
        assert!(second
            .unwrap_err()
            .message()
            .contains("unresolved invocation"));
        // The first one delivered again: the same task, nothing sent again.
        let replay = service
            .start_local_invocation(CODEX_THREAD_CAPABILITY, "native-agent:first", &input)
            .unwrap();
        assert_eq!(replay.task_id, first.task_id);
        // The same task id with other input is refused, not merged.
        let other = capability::codex::thread_continuation::continue_input(
            &workspace,
            "thread-1",
            "different",
            Some("turn-old"),
        )
        .unwrap();
        assert!(service
            .start_local_invocation(CODEX_THREAD_CAPABILITY, "native-agent:first", &other)
            .is_err());
        assert_eq!(turn_started(), 1, "nothing ran twice");
        // Stop ends the running turn; the folder is free again afterwards.
        service.cancel_task(&first.task_id).unwrap();
        let ended = wait_for_terminal(&service, &first.task_id);
        assert_eq!(ended.state, NativeAgentTaskStateV1::Cancelled, "{ended:?}");
        wait_for_workspace_release(&service, &workspace);
        assert_eq!(turn_started(), 1);
        let _ = fs::remove_dir_all(root);
    }
}

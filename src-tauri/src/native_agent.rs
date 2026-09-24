//! Native mature-Agent Host capabilities.
//!
//! This module is deliberately outside the managed Worker and object-flow
//! machinery.  A native Agent owns its provider, authentication, tools,
//! sandbox, workspace semantics, and conversation.  Pastey owns only the
//! bounded task envelope and the lifecycle it needs for orchestration.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_TASK_BYTES: usize = 16 * 1024;
/// Bounds one native control RPC acknowledgement only. It never bounds the
/// accepted native turn's execution or terminal observation.
const NATIVE_AGENT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const NATIVE_AGENT_PROTOCOL_SCHEMA: &str = "pastey-native-agent-control-v1";
pub(crate) const NATIVE_AGENT_TASK_SCHEMA: &str = "pastey-native-agent-task-v1";
pub(crate) const CODEX_CAPABILITY_ID: &str = "agent.coding.codex";
pub(crate) const NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA: &str =
    "pastey-native-agent-workspace-movement-v1";
/// The intentionally small, exact control surface Pastey 2.0 understands for
/// its one concrete native capability. This is a Host capability fact, never
/// execution, Transfer, or session authority.
pub(crate) const DIRECT_NATIVE_INVOKE_PROTOCOLS: [&str; 2] =
    [NATIVE_AGENT_PROTOCOL_SCHEMA, NATIVE_AGENT_TASK_SCHEMA];
pub(crate) const WORKSPACE_MOVEMENT_PROTOCOLS: [&str; 3] = [
    NATIVE_AGENT_PROTOCOL_SCHEMA,
    NATIVE_AGENT_TASK_SCHEMA,
    NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA,
];
const MAX_TASK_ID_BYTES: usize = 256;
const MAX_WORKSPACE_BYTES: usize = 4 * 1024;
const MAX_MOVEMENT_ID_BYTES: usize = 256;

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

#[derive(Clone, Deserialize, Serialize)]
struct PersistedNativeAgentEnvelopeV1 {
    task: NativeAgentTaskStatusV1,
    task_workspace: Option<PathBuf>,
    remote_target: Option<String>,
    task_digest: Option<String>,
    #[serde(default)]
    bridge_id: Option<String>,
    movement: Option<WorkspaceMovementRecordV1>,
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
}

struct NativeCodexSessionV1 {
    controller: Arc<CodexAppServerV1>,
    thread_id: String,
}

/// A small Host-private service, not an Agent registry or a Worker adapter.
#[derive(Default)]
pub(crate) struct NativeAgentServiceV1 {
    codex_sessions: HashMap<PathBuf, NativeCodexSessionV1>,
    tasks: Arc<Mutex<HashMap<String, NativeAgentTaskStatusV1>>>,
    active_workspaces: Arc<Mutex<HashMap<PathBuf, String>>>,
    remote_targets: HashMap<String, String>,
    task_workspaces: HashMap<String, PathBuf>,
    task_digests: HashMap<String, String>,
    task_bridges: HashMap<String, String>,
    revoked_bridges: HashSet<String>,
    workspace_movements: HashMap<String, WorkspaceMovementRecordV1>,
    durable_paths: Option<crate::storage::AppPaths>,
    #[cfg(test)]
    fail_post_start_persist_once: bool,
}

impl NativeAgentServiceV1 {
    pub(crate) fn with_paths(paths: crate::storage::AppPaths) -> AppResult<Self> {
        let mut service = Self {
            durable_paths: Some(paths),
            ..Self::default()
        };
        service.restore_durable_envelopes()?;
        Ok(service)
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
            let mut changed = false;
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
        if let Ok(task) = serde_json::to_value(status) {
            let _ =
                crate::storage::update_native_agent_observed_task_if_present(paths, task_id, &task);
        }
    }
    pub(crate) fn capabilities(&self) -> Vec<NativeAgentCapabilityV1> {
        vec![NativeAgentCapabilityV1 {
            agent_id: CODEX_CAPABILITY_ID.into(),
            display_name: "Codex".into(),
            state: codex_compatibility_at(Path::new("codex")),
        }]
    }

    pub(crate) fn native_capability_fact(&self) -> crate::peer_capabilities::HostCapabilityFact {
        crate::peer_capabilities::native_agent_capability_fact(codex_compatibility_at(Path::new(
            "codex",
        )))
    }

    pub(crate) fn require_codex_compatibility(&self) -> AppResult<()> {
        match codex_compatibility_at(Path::new("codex")) {
            NativeAgentCapabilityStateV1::Available => Ok(()),
            NativeAgentCapabilityStateV1::Incompatible => {
                invalid("Codex native app-server interface is incompatible.")
            }
            NativeAgentCapabilityStateV1::Unavailable => {
                invalid("Codex native capability is unavailable.")
            }
        }
    }

    /// Creates the user-visible Review envelope on the initiating Host.  This
    /// only captures an exact safe baseline; no package, transfer, or Agent
    /// invocation exists until the one approval below.
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
                "Pastey will send \"{source_workspace_name}\" to the selected device, let Codex work on it, then return the result here."
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
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        let status = self.propose_workspace_movement(
            movement_id,
            task_id,
            source_workspace,
            target_host_ref,
            source_object,
            task,
            resume,
        )?;
        self.task_bridges
            .insert(task_id.to_owned(), bridge_id.to_owned());
        if let Some(record) = self.workspace_movements.get_mut(movement_id) {
            record.bridge_id = Some(bridge_id.to_owned());
        }
        if let Err(error) = self.persist_envelope(task_id, None) {
            self.task_bridges.remove(task_id);
            if let Some(record) = self.workspace_movements.get_mut(movement_id) {
                record.bridge_id = None;
            }
            return Err(error);
        }
        Ok(status)
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
        if source_workspace
            .bridge_id
            .as_deref()
            .is_some_and(|bound| bound != bridge_id)
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
                },
            );
        self.task_digests
            .insert(request.task_id.clone(), task_digest.clone());
        self.persist_envelope(&request.task_id, Some(&task_digest))?;
        Ok(())
    }

    pub(crate) fn accept_bridge_workspace_prepare(
        &mut self,
        bridge_id: &str,
        request: NativeAgentWorkspacePrepareV1,
    ) -> AppResult<()> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        let task_id = request.task_id.clone();
        let movement_id = request.movement_id.clone();
        self.accept_workspace_prepare(request)?;
        self.task_bridges
            .insert(task_id.clone(), bridge_id.to_owned());
        if let Some(record) = self.workspace_movements.get_mut(&movement_id) {
            record.bridge_id = Some(bridge_id.to_owned());
        }
        self.persist_envelope(&task_id, None)
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

    /// Binds the received file-set to the pre-authorized task workspace.  The
    /// native Agent sees only that ordinary local directory.
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

    fn start_received_workspace_task_with_executable(
        &mut self,
        executable: &Path,
        movement_id: &str,
        task_workspace: &Path,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        let (task_id, task, resume) = {
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
        if !resume && self.codex_sessions.contains_key(task_workspace) {
            return invalid(
                "Native Agent workspace already has a session; resumption is required.",
            );
        }
        let canonical_workspace = task_workspace.canonicalize().map_err(|_| {
            AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
        })?;
        // Commit ownership of the materialized tree while the task is still
        // queued. If this write fails, no native turn has started and landing
        // cleanup may safely remove the tree.
        self.workspace_movements
            .get_mut(movement_id)
            .expect("checked above")
            .task_workspace = Some(canonical_workspace.clone());
        if let Err(error) = self.persist_envelope(&task_id, None) {
            self.workspace_movements
                .get_mut(movement_id)
                .expect("checked above")
                .task_workspace = None;
            return Err(error);
        }
        // The queued entry is only the pre-transfer envelope, not proof of a
        // native turn. Replace it exactly once after the tree is durable.
        let queued = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .remove(&task_id);
        let had_session = self.codex_sessions.contains_key(&canonical_workspace);
        let started = self.start_codex_task_with_executable_and_id_in_movement(
            executable,
            &task_id,
            &canonical_workspace,
            &task,
            Some(movement_id),
        );
        let status = match started {
            Ok(status) => status,
            Err(error) => {
                // No turn was spawned on this error path. Close an app-server
                // created for this landing before its tree may be cleaned.
                if !had_session {
                    if let Some(session) = self.codex_sessions.remove(&canonical_workspace) {
                        session.controller.shutdown();
                    }
                }
                self.task_workspaces.remove(&task_id);
                if let Some(queued) = queued {
                    self.tasks
                        .lock()
                        .map_err(|_| {
                            AppError::InvalidInput("Native Agent task store is unavailable.".into())
                        })?
                        .insert(task_id.clone(), queued);
                }
                self.workspace_movements
                    .get_mut(movement_id)
                    .expect("checked above")
                    .task_workspace = None;
                let _ = self.persist_envelope(&task_id, None);
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
                .active_workspaces
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent session store is unavailable.".into())
                })?
                .contains_key(workspace);
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
            if let Some(session) = self.codex_sessions.remove(workspace) {
                session.controller.shutdown();
            }
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

    pub(crate) fn start_codex_task(
        &mut self,
        workspace: &Path,
        task: &str,
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
        self.start_codex_task_with_id(
            &format!("native-agent:{}", Uuid::new_v4()),
            &workspace,
            task,
        )
    }

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
            if self.task_workspaces.get(task_id) != Some(&workspace)
                || self.task_digests.get(task_id) != Some(&Self::task_digest(task))
            {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        if !resume && self.codex_sessions.contains_key(&workspace) {
            return invalid(
                "Native Agent workspace already has a session; resumption is required.",
            );
        }
        self.start_codex_task_with_executable_and_id(Path::new("codex"), task_id, &workspace, task)
    }

    pub(crate) fn start_bridge_codex_task_with_id_resume(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        workspace: &Path,
        task: &str,
        resume: bool,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        self.task_bridges
            .insert(task_id.to_owned(), bridge_id.to_owned());
        match self.start_codex_task_with_id_resume(task_id, workspace, task, resume) {
            Ok(status) => Ok(status),
            Err(error) => {
                self.task_bridges.remove(task_id);
                Err(error)
            }
        }
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

    fn start_codex_task_with_executable_and_id(
        &mut self,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        self.start_codex_task_with_executable_and_id_in_movement(
            executable, task_id, workspace, task, None,
        )
    }

    fn start_codex_task_with_executable_and_id_in_movement(
        &mut self,
        executable: &Path,
        task_id: &str,
        workspace: &Path,
        task: &str,
        movement_id: Option<&str>,
    ) -> AppResult<NativeAgentTaskStatusV1> {
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
            if self.task_workspaces.get(task_id) != Some(&workspace)
                || self.task_digests.get(task_id) != Some(&Self::task_digest(task))
            {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        match codex_compatibility_at(executable) {
            NativeAgentCapabilityStateV1::Available => {}
            NativeAgentCapabilityStateV1::Incompatible => {
                return invalid(
                    "Codex is detected but its native app-server interface is incompatible.",
                )
            }
            NativeAgentCapabilityStateV1::Unavailable => {
                return invalid("Codex native capability is unavailable.")
            }
        }
        if self.workspace_has_authoritative_owner(&workspace, None, movement_id)? {
            return invalid("Codex already has a running task in this workspace session.");
        }

        let (controller, thread_id, session_reused) = match self.codex_sessions.get(&workspace) {
            Some(session) => (session.controller.clone(), session.thread_id.clone(), true),
            None => {
                let controller = Arc::new(CodexAppServerV1::launch(executable, &workspace)?);
                let thread_id = controller.start_thread(&workspace)?;
                self.codex_sessions.insert(
                    workspace.clone(),
                    NativeCodexSessionV1 {
                        controller: controller.clone(),
                        thread_id: thread_id.clone(),
                    },
                );
                (controller, thread_id, false)
            }
        };
        let task_id = task_id.to_owned();
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: task_id.clone(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: workspace
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
                .into(),
            session_reused,
            state: NativeAgentTaskStateV1::Running,
            result: None,
            code: None,
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.clone(), status.clone());
        self.task_workspaces
            .insert(task_id.clone(), workspace.clone());
        self.task_digests
            .insert(task_id.clone(), Self::task_digest(task));
        self.persist_envelope(&task_id, Some(&Self::task_digest(task)))?;
        self.active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .insert(workspace.clone(), task_id.clone());
        let tasks = self.tasks.clone();
        let durable_paths = self.durable_paths.clone();
        let active_workspaces = self.active_workspaces.clone();
        let completed_workspace = workspace.clone();
        let prompt = task.to_owned();
        thread::spawn(move || {
            let outcome = controller.run_turn(&thread_id, &prompt);
            let mut persisted_status = None;
            if let Ok(mut tasks) = tasks.lock() {
                if let Some(status) = tasks.get_mut(&task_id) {
                    // Cancellation wins a concurrent late native completion.
                    // The observer still reaches this point to release its
                    // Host-private workspace execution occupancy.
                    if status.state == NativeAgentTaskStateV1::Cancelled {
                        if outcome == NativeTurnOutcomeV1::Cancelled {
                            status.code = Some("native_agent_cancelled".into());
                        }
                    } else {
                        match outcome {
                            NativeTurnOutcomeV1::Completed => {
                                status.state = NativeAgentTaskStateV1::Completed;
                                status.result = Some("Codex completed its native task.".into());
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
                        }
                    }
                    persisted_status = Some(status.clone());
                }
            }
            if let (Some(paths), Some(status)) = (durable_paths.as_ref(), persisted_status.as_ref())
            {
                NativeAgentServiceV1::persist_task_status_after_native_turn(
                    paths, &task_id, status,
                );
            }
            if outcome == NativeTurnOutcomeV1::Unknown {
                // A turn/start acknowledgement or terminal shape may be
                // indeterminate while the app-server remains alive. Keep the
                // workspace occupied until its observation channel is actually
                // lost; a different task must not overlap possible execution.
                controller.wait_until_observation_lost();
            }
            // Always release only after the native observer exits, including
            // when cancellation had already revoked visible authority.
            active_workspaces
                .lock()
                .ok()
                .map(|mut active| active.remove(&completed_workspace));
        });
        Ok(status)
    }

    /// The one Host-local occupancy check shared by direct Agent runs and
    /// approved workspace movement. A canonical workspace remains owned while
    /// any execution, movement, uncertain outcome, or apply recovery can still
    /// affect it.
    fn workspace_has_authoritative_owner(
        &self,
        workspace: &Path,
        except_task_id: Option<&str>,
        except_movement_id: Option<&str>,
    ) -> AppResult<bool> {
        let canonical = workspace
            .canonicalize()
            .map_err(|_| AppError::InvalidInput("Native Agent workspace is unavailable.".into()))?;
        if self
            .active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .keys()
            .any(|occupied| occupied == &canonical)
        {
            return Ok(true);
        }
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
                        .is_some_and(|owned| owned == &canonical)
            })
        {
            return Ok(true);
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
                && self
                    .task_workspaces
                    .get(task_id)
                    .is_some_and(|owned| owned == &canonical)
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
    /// workspace occupancy remains governed by `active_workspaces`.
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

    pub(crate) fn queue_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty()
            || target_host_ref.trim().is_empty()
            || workspace.trim().is_empty()
            || task.trim().is_empty()
        {
            return invalid("Remote native Agent task is invalid.");
        }
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if self.remote_targets.get(task_id).map(String::as_str) != Some(target_host_ref)
                || self.task_workspaces.get(task_id) != Some(&PathBuf::from(workspace))
                || self.task_digests.get(task_id) != Some(&Self::task_digest(task))
            {
                return invalid(
                    "Remote native Agent identity was reused with conflicting correlation.",
                );
            }
            return Ok(existing);
        }
        let status = NativeAgentTaskStatusV1 {
            schema_version: "pastey-native-agent-task-v1".into(),
            task_id: task_id.into(),
            agent_id: CODEX_CAPABILITY_ID.into(),
            workspace_name: workspace
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("workspace")
                .into(),
            session_reused: false,
            state: NativeAgentTaskStateV1::Queued,
            result: None,
            code: Some("remote_agent_queued".into()),
        };
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .insert(task_id.into(), status.clone());
        self.remote_targets
            .insert(task_id.into(), target_host_ref.into());
        self.task_workspaces
            .insert(task_id.into(), PathBuf::from(workspace));
        self.task_digests
            .insert(task_id.into(), Self::task_digest(task));
        self.persist_envelope(task_id, Some(&Self::task_digest(task)))?;
        Ok(status)
    }

    pub(crate) fn queue_bridge_remote_task(
        &mut self,
        bridge_id: &str,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
        task: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if self.revoked_bridges.contains(bridge_id) {
            return invalid("Native Agent Bridge authority was revoked.");
        }
        self.task_bridges
            .insert(task_id.to_owned(), bridge_id.to_owned());
        match self.queue_remote_task(task_id, target_host_ref, workspace, task) {
            Ok(status) => Ok(status),
            Err(error) => {
                self.task_bridges.remove(task_id);
                Err(error)
            }
        }
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
                task.code = Some("native_agent_reconciliation_required".into());
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
        if let Some(workspace) = self.task_workspaces.get(task_id) {
            if let Some(session) = self.codex_sessions.get(workspace) {
                let _ = session.controller.cancel_owned_turn_or_session();
            }
        }
        Ok(status)
    }

    /// Records Bridge-scoped uncertainty after route loss without interrupting
    /// Host-local Codex sessions or live observers. Durable task, movement,
    /// result snapshot, retained conflict, and received workspace facts remain
    /// available for authenticated fresh-session reconciliation. Burn uses
    /// `purge_bridge_authority` below.
    pub(crate) fn revoke_bridge_session(&mut self, bridge_id: &str) -> AppResult<()> {
        let task_ids = self
            .task_bridges
            .iter()
            .filter_map(|(task_id, bound)| (bound == bridge_id).then_some(task_id.clone()))
            .collect::<Vec<_>>();
        let actively_observed_tasks = self
            .active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .values()
            .cloned()
            .collect::<HashSet<_>>();
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
                        task.code = Some("native_agent_reconciliation_required".into());
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
    pub(crate) fn purge_bridge_authority(&mut self, bridge_id: &str) -> AppResult<()> {
        self.revoked_bridges.insert(bridge_id.to_owned());
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
                }
            }
        }
        for task_id in &task_ids {
            if let Some(workspace) = self.task_workspaces.get(task_id).cloned() {
                if let Some(session) = self.codex_sessions.remove(&workspace) {
                    let _ = session.controller.cancel_owned_turn_or_session();
                }
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
            self.task_bridges.remove(&task_id);
        }
        Ok(())
    }

    pub(crate) fn cancel_task(&mut self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        let workspace = self.task_workspaces.get(task_id).cloned();
        let (status, native_execution_needs_cancellation) = {
            let mut tasks = self.tasks.lock().map_err(|_| {
                AppError::InvalidInput("Native Agent task store is unavailable.".into())
            })?;
            let task = tasks.get_mut(task_id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent task is unavailable.".into())
            })?;
            let uncertain_active_turn = task.state == NativeAgentTaskStateV1::Interrupted
                && task.code.as_deref() == Some("native_agent_outcome_unknown");
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
        let workspace = workspace.ok_or_else(|| {
            AppError::InvalidInput("Native Agent task workspace is unavailable.".into())
        })?;
        let termination_path = self
            .codex_sessions
            .get(&workspace)
            .map(|session| session.controller.cancel_owned_turn_or_session())
            .unwrap_or_else(|| invalid("Native Agent session is unavailable."));
        let terminate_session = match termination_path {
            Ok(terminate_session) => terminate_session,
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
                return Ok(status);
            }
        };
        if terminate_session {
            // The observer retains its Arc long enough to see the terminated
            // app-server channel and release occupancy, but this service must
            // never offer that dead controller to a later task.
            self.codex_sessions.remove(&workspace);
        }
        Ok(status)
    }

    pub(crate) fn shutdown(&mut self) {
        for session in self.codex_sessions.values() {
            session.controller.shutdown();
        }
        self.codex_sessions.clear();
        self.active_workspaces
            .lock()
            .ok()
            .map(|mut active| active.clear());
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
            let service = runtime.native_agents.lock();
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
                    if let Ok(event) = crate::room_control::native_agent_event(
                        "native_agent.status",
                        payload,
                        &context,
                    ) {
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

pub(crate) fn validate_status(status: &NativeAgentStatusV1) -> AppResult<()> {
    if status.schema_version != NATIVE_AGENT_PROTOCOL_SCHEMA
        || status.task_id.trim().is_empty()
        || status.task_id.len() > MAX_TASK_ID_BYTES
        || status.executing_host_ref.trim().is_empty()
        || status.executing_host_ref.len() > 256
        || status.status.schema_version != "pastey-native-agent-task-v1"
        || status.status.task_id != status.task_id
        || status.status.agent_id != CODEX_CAPABILITY_ID
        || status.status.workspace_name.trim().is_empty()
        || status.status.workspace_name.len() > 256
        || status.status.workspace_name.contains('/')
        || status.status.workspace_name.contains('\\')
        || !valid_lifecycle_code(status.status.code.as_deref())
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

fn turn_start_ack_timeout() -> Duration {
    #[cfg(test)]
    {
        // A controlled test-only RPC bound proves acknowledgement ambiguity
        // without turning a unit test into a 30-second wait. It is not an
        // execution-duration limit.
        Duration::from_millis(50)
    }
    #[cfg(not(test))]
    {
        NATIVE_AGENT_RPC_TIMEOUT
    }
}

fn codex_compatibility_at(executable: &Path) -> NativeAgentCapabilityStateV1 {
    let detected = Command::new(executable)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !detected {
        return NativeAgentCapabilityStateV1::Unavailable;
    }
    let app_server_usable = Command::new(executable)
        .args(["app-server", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if app_server_usable {
        NativeAgentCapabilityStateV1::Available
    } else {
        // Detection proves only the native product exists. The app-server
        // compatibility fact is deliberately limited to Pastey's fixed
        // initialize/thread/start/turn/start/observation/interrupt/shutdown
        // surface; no provider, model, credential, or native session detail
        // is queried here.
        NativeAgentCapabilityStateV1::Incompatible
    }
}

struct CodexAppServerV1 {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    messages: Mutex<mpsc::Receiver<AppResult<Value>>>,
    active_turn: Mutex<Option<(String, String)>>,
    interrupt_requested: AtomicBool,
}

impl CodexAppServerV1 {
    fn launch(executable: &Path, workspace: &Path) -> AppResult<Self> {
        // Deliberately inherit the user's native Codex environment, including
        // authentication and any native configuration. Pastey owns none of it.
        let mut child = Command::new(executable)
            .args(["app-server", "--stdio"])
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stdin is unavailable.".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stdout is unavailable.".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::InvalidInput("Codex stderr is unavailable.".into()))?;
        let (sender, receiver) = mpsc::sync_channel(256);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) if line.len() > MAX_LINE_BYTES => {
                        let _ = sender.send(invalid("Codex app-server line exceeded its bound."));
                        return;
                    }
                    Ok(_) => {
                        let _ = sender.send(serde_json::from_str(&line).map_err(AppError::from));
                    }
                    Err(error) => {
                        let _ = sender.send(Err(AppError::Io(error)));
                        return;
                    }
                }
            }
        });
        thread::spawn(move || {
            let _ = BufReader::new(stderr)
                .take(MAX_STDERR_BYTES as u64)
                .read_to_end(&mut Vec::new());
        });
        let controller = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            messages: Mutex::new(receiver),
            active_turn: Mutex::new(None),
            interrupt_requested: AtomicBool::new(false),
        };
        controller.send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        controller.await_result(1, Instant::now() + NATIVE_AGENT_RPC_TIMEOUT)?;
        controller.send(json!({"method": "initialized", "params": {}}))?;
        Ok(controller)
    }

    fn start_thread(&self, workspace: &Path) -> AppResult<String> {
        self.send(json!({"id": 2, "method": "thread/start", "params": {"cwd": workspace}}))?;
        self.await_result(2, Instant::now() + NATIVE_AGENT_RPC_TIMEOUT)?
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a native session.".into()))
    }

    /// Obtains the exact turn identity with a bounded RPC timeout, then waits
    /// indefinitely for that turn's native terminal fact. Silence is not a
    /// lifecycle event; only channel/process loss is unknown.
    fn run_turn(&self, thread_id: &str, task: &str) -> NativeTurnOutcomeV1 {
        if self
            .send(json!({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": task}]}}))
            .is_err()
        {
            return NativeTurnOutcomeV1::Unknown;
        }
        let turn = match self.await_result(3, Instant::now() + turn_start_ack_timeout()) {
            Ok(turn) => turn,
            // The request may have reached Codex even though Pastey did not
            // receive its acknowledgement. Never retry or infer failure.
            Err(_) => return NativeTurnOutcomeV1::Unknown,
        };
        let Some(turn_id) = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return NativeTurnOutcomeV1::Unknown;
        };
        if self
            .active_turn
            .lock()
            .map(|mut active| *active = Some((thread_id.into(), turn_id.clone())))
            .is_err()
        {
            return NativeTurnOutcomeV1::Unknown;
        }
        // Cancellation may have been requested while turn/start was awaiting
        // its acknowledgement. Once its exact identity is known, issue the
        // one bounded native interrupt without clearing occupancy.
        let _ = self.send_active_interrupt_if_requested();
        let outcome = loop {
            match self.next_observation() {
                Ok(message)
                    if message.get("method").and_then(Value::as_str) == Some("turn/completed") =>
                {
                    break codex_completed_turn_outcome(&message, thread_id, &turn_id);
                }
                Ok(message) if message.get("error").is_some() => {
                    break NativeTurnOutcomeV1::Unknown;
                }
                Ok(_) => {}
                // App-server stdout/process observation disappeared before an
                // exact terminal outcome. This is indeterminate, not failed.
                Err(_) => break NativeTurnOutcomeV1::Unknown,
            }
        };
        let _ = self.clear_active_turn();
        outcome
    }

    fn clear_active_turn(&self) -> AppResult<()> {
        *self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))? =
            None;
        self.interrupt_requested.store(false, Ordering::Release);
        Ok(())
    }

    /// Records cancellation intent independently from native termination. The
    /// exact active turn stays retained until the observer exits.
    fn interrupt(&self) -> AppResult<()> {
        self.interrupt_requested.store(true, Ordering::Release);
        self.send_active_interrupt_if_requested()
    }

    /// Explicit user cancellation may arrive after `turn/start` was accepted
    /// but before Pastey obtained its exact turn identity. In that one case,
    /// `turn/interrupt` is not available, so terminate this Host-private
    /// app-server/session instead. This is never automatic: ordinary unknown
    /// outcome handling keeps observing and retains workspace occupancy.
    fn cancel_owned_turn_or_session(&self) -> AppResult<bool> {
        self.interrupt_requested.store(true, Ordering::Release);
        let has_exact_turn = self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))?
            .is_some();
        if has_exact_turn {
            self.send_active_interrupt_if_requested()?;
            return Ok(false);
        }
        self.shutdown();
        Ok(true)
    }

    fn send_active_interrupt_if_requested(&self) -> AppResult<()> {
        if !self.interrupt_requested.load(Ordering::Acquire) {
            return Ok(());
        }
        let active = self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))?
            .clone();
        if let Some((thread_id, turn_id)) = active {
            self.send(json!({"id": 4, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}))?;
        }
        Ok(())
    }
    fn shutdown(&self) {
        let _ = self.interrupt();
        let _ = self.send(json!({"id": 5, "method": "shutdown", "params": {}}));
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn send(&self, value: Value) -> AppResult<()> {
        let encoded = serde_json::to_vec(&value)?;
        if encoded.len() > MAX_LINE_BYTES {
            return invalid("Codex request exceeds its bound.");
        }
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex stdin is unavailable.".into()))?;
        stdin.write_all(&encoded)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
    fn await_result(&self, id: u64, deadline: Instant) -> AppResult<Value> {
        loop {
            let message = self.next_message(deadline)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message.get("result").cloned().ok_or_else(|| {
                    AppError::InvalidInput("Codex app-server request failed.".into())
                });
            }
            if message.get("error").is_some() {
                return invalid("Codex app-server request failed.");
            }
        }
    }
    fn next_message(&self, deadline: Instant) -> AppResult<Value> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| AppError::InvalidInput("Codex native task timed out.".into()))?;
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv_timeout(remaining)
            .map_err(|_| AppError::InvalidInput("Codex native task outcome is unknown.".into()))?
    }

    fn next_observation(&self) -> AppResult<Value> {
        self.messages
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex messages are unavailable.".into()))?
            .recv()
            .map_err(|_| {
                AppError::InvalidInput("Codex native observation is unavailable.".into())
            })?
    }

    fn wait_until_observation_lost(&self) {
        while self.next_observation().is_ok() {}
    }
}

impl Drop for CodexAppServerV1 {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

/// The native app-server can emit terminal notifications for other threads on
/// the same connection.  A Pastey task reaches success only when the terminal
/// notification names the exact `thread/start` thread and `turn/start` turn,
/// and Codex reports that turn as completed without an error.  Every other
/// terminal shape is intentionally non-success so it cannot trigger a result
/// scan, Return Transfer, apply, or global DONE.
fn codex_completed_turn_outcome(
    message: &Value,
    thread_id: &str,
    turn_id: &str,
) -> NativeTurnOutcomeV1 {
    let Some(params) = message.get("params").and_then(Value::as_object) else {
        return NativeTurnOutcomeV1::Unknown;
    };
    if params.get("threadId").and_then(Value::as_str) != Some(thread_id) {
        return NativeTurnOutcomeV1::Unknown;
    }
    let Some(turn) = params.get("turn").and_then(Value::as_object) else {
        return NativeTurnOutcomeV1::Unknown;
    };
    if turn.get("id").and_then(Value::as_str) != Some(turn_id) {
        return NativeTurnOutcomeV1::Unknown;
    }
    match turn.get("status").and_then(Value::as_str) {
        Some("completed") if turn.get("error").is_none_or(Value::is_null) => {
            NativeTurnOutcomeV1::Completed
        }
        Some("completed") => NativeTurnOutcomeV1::Unknown,
        Some("failed") => NativeTurnOutcomeV1::Failed,
        Some("interrupted") => NativeTurnOutcomeV1::Interrupted,
        // `cancelled` is not currently emitted by Codex's schema, but treating
        // it as an explicit non-success protects this boundary across native
        // protocol versions and lets the task envelope surface cancellation.
        Some("cancelled") => NativeTurnOutcomeV1::Cancelled,
        Some("inProgress") => NativeTurnOutcomeV1::Unknown,
        _ => NativeTurnOutcomeV1::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

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
        let mut permissions = fs::metadata(&agent).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&agent, permissions).unwrap();
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
if [ "$2" = "--help" ]; then exit 0; fi
while IFS= read -r line; do
  case "$line" in
    *'"id":1'*) echo '{"id":1,"result":{}}' ;;
    *'"id":2'*) echo '{"id":2,"result":{"thread":{"id":"native-thread"}}}' ;;
    *'"id":3'*)
      echo '{"id":3,"result":{"turn":{"id":"native-turn"}}}'
      touch "$PWD/turn-started"
      while [ ! -f "$PWD/release-turn" ]; do sleep 0.01; done
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
        let mut permissions = fs::metadata(&agent).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&agent, permissions).unwrap();
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
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        executable
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
        for _ in 0..300 {
            if !service
                .active_workspaces
                .lock()
                .unwrap()
                .contains_key(workspace)
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture native Agent did not release its workspace")
    }

    fn wait_for_live_terminal(
        service: &NativeAgentServiceV1,
        task_id: &str,
    ) -> NativeAgentTaskStatusV1 {
        for _ in 0..720 {
            let status = service.task_status(task_id).unwrap();
            if status.state != NativeAgentTaskStateV1::Running {
                return status;
            }
            thread::sleep(Duration::from_millis(250));
        }
        panic!("installed Codex did not return a bounded terminal outcome")
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
        let mut service = NativeAgentServiceV1::default();
        let first = service
            .start_codex_task(
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
            .start_codex_task(
                &workspace,
                "In the file you created in the preceding turn, append exactly one new line: `second native turn`. Make the edit now.",
            )
            .unwrap();
        assert!(second.session_reused);
        assert_eq!(
            wait_for_live_terminal(&service, &second.task_id).state,
            NativeAgentTaskStateV1::Completed
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "first native turn\nsecond native turn\n"
        );
        service.shutdown();
        let _ = fs::remove_dir_all(root);
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
            .active_workspaces
            .lock()
            .unwrap()
            .contains_key(&workspace.canonicalize().unwrap()));

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
            .accept_workspace_prepare(NativeAgentWorkspacePrepareV1 {
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
            })
            .unwrap();
        let started = service
            .start_received_workspace_task_with_executable(
                &agent,
                "movement-cancel-host",
                &workspace,
            )
            .unwrap();
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
            .propose_workspace_movement(
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
            .propose_workspace_movement(
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
            .accept_bridge_workspace_prepare("room-conflict", prepare)
            .unwrap();
        executor
            .start_received_workspace_task_with_executable(&agent, "movement-conflict", &received)
            .unwrap();
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
            .accept_bridge_workspace_prepare(
                bridge_id,
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
                .propose_workspace_movement(
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
        service.accept_workspace_prepare(request.clone()).unwrap();
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
        service
            .start_received_workspace_task_with_executable(&agent, "movement-landing", &landed)
            .unwrap();
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
            .accept_bridge_workspace_prepare("room-session-loss", request)
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
        service
            .task_bridges
            .insert("task-live-turn".into(), "room-live-turn".into());
        let started = service
            .start_codex_task_with_executable_and_id(
                &agent,
                "task-live-turn",
                &workspace,
                "finish this turn after the route is lost",
            )
            .unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();
        let controller = service
            .codex_sessions
            .get(&canonical_workspace)
            .unwrap()
            .controller
            .clone();
        for _ in 0..300 {
            if controller.active_turn.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.active_turn.lock().unwrap().is_some());
        assert!(workspace.join("turn-started").exists());
        assert_eq!(
            service
                .active_workspaces
                .lock()
                .unwrap()
                .get(&canonical_workspace)
                .map(String::as_str),
            Some(started.task_id.as_str())
        );

        service.revoke_bridge_session("room-live-turn").unwrap();

        assert_eq!(
            service.task_status("task-live-turn").unwrap().state,
            NativeAgentTaskStateV1::Running
        );
        assert!(service.codex_sessions.contains_key(&canonical_workspace));
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
        assert!(service.codex_sessions.contains_key(&canonical_workspace));
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
            .accept_bridge_workspace_prepare(
                "room-completed-session-loss",
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
            .accept_bridge_workspace_prepare(
                bridge_id,
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
        let started = service
            .start_received_workspace_task_with_executable(&agent, movement_id, &task_workspace)
            .unwrap();
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
        assert!(recovered.codex_sessions.is_empty());
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
            .accept_bridge_workspace_prepare(
                "room-artifacts",
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
            .accept_bridge_workspace_prepare(
                "room-artifacts",
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
                .accept_bridge_workspace_prepare("room-artifacts", prepare(movement_id, task_id))
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
            .accept_bridge_workspace_prepare(
                "room-retry",
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
            .accept_bridge_workspace_prepare(
                "room-no-route",
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

    #[test]
    fn prepared_workspace_cancel_is_idempotent_and_blocks_late_landing() {
        let root =
            std::env::temp_dir().join(format!("pastey-native-prepare-cancel-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut service = NativeAgentServiceV1::with_paths(paths).unwrap();
        service
            .accept_bridge_workspace_prepare(
                "room",
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

        let first = service.cancel_task("task-prepared").unwrap();
        let duplicate = service.cancel_task("task-prepared").unwrap();
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
        assert!(service.active_workspaces.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_bridge_stop_cancels_recovered_target_side_authority_without_a_route() {
        let root = std::env::temp_dir().join(format!("pastey-native-stop-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .accept_bridge_workspace_prepare(
                "room-recovery",
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
            .accept_bridge_workspace_prepare("room-finish", prepare)
            .unwrap();
        // Receiver /finish has completed landing and started the one Agent
        // turn before this sender-local UI write fails.
        executor
            .start_received_workspace_task_with_executable(&agent, "movement-finish", &received)
            .unwrap();
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
        service
            .task_bridges
            .insert("task-burn-observer".into(), "room-burn".into());
        service
            .start_codex_task_with_executable_and_id(
                &agent,
                "task-burn-observer",
                &workspace,
                "wait",
            )
            .unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();
        let controller = service
            .codex_sessions
            .get(&canonical_workspace)
            .unwrap()
            .controller
            .clone();
        for _ in 0..300 {
            if controller.active_turn.lock().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(controller.active_turn.lock().unwrap().is_some());
        service.purge_bridge_authority("room-burn").unwrap();
        assert!(service.revoked_bridges.contains("room-burn"));
        assert!(!service.codex_sessions.contains_key(&canonical_workspace));
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
            .accept_bridge_workspace_prepare("room-exact", prepare)
            .unwrap();
        executor
            .start_received_workspace_task_with_executable(
                &agent,
                "movement-exact",
                &task_workspace,
            )
            .unwrap();
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
            .accept_bridge_workspace_prepare("room-unrepresentable", prepare)
            .unwrap();
        executor
            .start_received_workspace_task_with_executable(
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
            .propose_workspace_movement(
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
            .accept_bridge_workspace_prepare(
                "room-persist",
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
        service
            .start_received_workspace_task_with_executable(&agent, "movement-persist", &workspace)
            .unwrap();
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
}

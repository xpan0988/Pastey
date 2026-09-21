//! Native mature-Agent Host capabilities.
//!
//! This module is deliberately outside the managed Worker and object-flow
//! machinery.  A native Agent owns its provider, authentication, tools,
//! sandbox, workspace semantics, and conversation.  Pastey owns only the
//! bounded task envelope and the lifecycle it needs for orchestration.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
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
const MAX_TASK_WALL_TIME: Duration = Duration::from_secs(15 * 60);
pub(crate) const NATIVE_AGENT_PROTOCOL_SCHEMA: &str = "pastey-native-agent-control-v1";
pub(crate) const CODEX_CAPABILITY_ID: &str = "agent.coding.codex";
pub(crate) const NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA: &str =
    "pastey-native-agent-workspace-movement-v1";
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
    bridge_id: Option<String>,
    apply_completed: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct PersistedNativeAgentEnvelopeV1 {
    task: NativeAgentTaskStatusV1,
    task_workspace: Option<PathBuf>,
    remote_target: Option<String>,
    task_digest: Option<String>,
    movement: Option<WorkspaceMovementRecordV1>,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAgentCapabilityV1 {
    pub(crate) agent_id: String,
    pub(crate) display_name: String,
    pub(crate) detected: bool,
    pub(crate) usable: bool,
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
    workspace_movements: HashMap<String, WorkspaceMovementRecordV1>,
    durable_paths: Option<crate::storage::AppPaths>,
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
        for stored in crate::storage::list_native_agent_envelopes(&paths)? {
            let mut persisted: PersistedNativeAgentEnvelopeV1 =
                serde_json::from_str(&stored.record_json).map_err(AppError::from)?;
            let mut changed = false;
            if matches!(
                persisted.task.state,
                NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
            ) {
                persisted.task.state = NativeAgentTaskStateV1::Interrupted;
                persisted.task.code = Some("native_agent_reconciliation_required".into());
                changed = true;
            }
            if let Some(movement) = persisted.movement.as_mut() {
                if !matches!(
                    movement.status.state,
                    NativeAgentWorkspaceMovementStateV1::AwaitingApproval
                        | NativeAgentWorkspaceMovementStateV1::Completed
                        | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                        | NativeAgentWorkspaceMovementStateV1::Failed
                        | NativeAgentWorkspaceMovementStateV1::Cancelled
                ) {
                    // A snapshotted result remains deliverable, but never
                    // assumes that an unobserved apply or native turn happened.
                    if movement.result_snapshot.is_some()
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
                self.workspace_movements
                    .insert(movement.status.movement_id.clone(), movement.clone());
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
            self.tasks
                .lock()
                .map_err(|_| {
                    AppError::InvalidInput("Native Agent task store is unavailable.".into())
                })?
                .insert(persisted.task.task_id.clone(), persisted.task.clone());
            if changed {
                self.persist_envelope(&persisted.task.task_id, persisted.task_digest.as_deref())?;
            }
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
            movement: movement.clone(),
        };
        let immutable_correlation = serde_json::to_string(&json!({
            "taskId": task_id,
            "movementId": movement.as_ref().map(|value| &value.status.movement_id),
            "targetHostRef": movement.as_ref().map(|value| &value.status.target_host_ref)
                .or_else(|| persisted.remote_target.as_ref()),
            "workspace": persisted.task_workspace,
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
        let Ok(Some(stored)) = crate::storage::get_native_agent_envelope(paths, task_id) else {
            return;
        };
        let Ok(mut persisted) =
            serde_json::from_str::<PersistedNativeAgentEnvelopeV1>(&stored.record_json)
        else {
            return;
        };
        persisted.task = status.clone();
        let _ = crate::storage::save_native_agent_envelope(
            paths,
            &crate::storage::StoredNativeAgentEnvelope {
                task_id: stored.task_id,
                movement_id: stored.movement_id,
                immutable_correlation: stored.immutable_correlation,
                record_json: serde_json::to_string(&persisted).unwrap_or_default(),
                updated_at: crate::storage::now_ts(),
            },
        );
    }
    pub(crate) fn capabilities(&self) -> Vec<NativeAgentCapabilityV1> {
        let detected = codex_detected();
        let usable = codex_usable();
        vec![NativeAgentCapabilityV1 {
            agent_id: CODEX_CAPABILITY_ID.into(),
            display_name: "Codex".into(),
            detected,
            usable,
        }]
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
                bridge_id: None,
                apply_completed: false,
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
        self.persist_envelope(task_id, Some(&Self::task_digest(task)))?;
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
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        if record.status.state != NativeAgentWorkspaceMovementStateV1::AwaitingApproval {
            return invalid("Native Agent workspace movement is not awaiting Review approval.");
        }
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
        self.persist_envelope(&task_id, None)?;
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
                bridge_id: None,
                apply_completed: false,
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
        self.persist_envelope(&request.task_id, Some(&task_digest))?;
        Ok(())
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
                if metadata.destination_host_ref != local_host_ref
                    || metadata.source_host_ref != record.status.target_host_ref
                    || record.status.state != NativeAgentWorkspaceMovementStateV1::ReturningResult
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

    /// Binds the received file-set to the pre-authorized task workspace.  The
    /// native Agent sees only that ordinary local directory.
    pub(crate) fn start_received_workspace_task(
        &mut self,
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
        // This queued entry is only the durable pre-transfer envelope; it is
        // not proof that a native turn started. Replace it exactly once when
        // the authenticated Transfer landing is materialized.
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .remove(&task_id);
        let status =
            self.start_codex_task_with_id_resume(&task_id, task_workspace, &task, resume)?;
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        record.status.state = NativeAgentWorkspaceMovementStateV1::AgentRunning;
        record.task_workspace = Some(task_workspace.to_path_buf());
        let task_id = record.status.task_id.clone();
        let _ = record;
        self.persist_envelope(&task_id, None)?;
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
            if let (Some(snapshot), Some(identity), Some(prepared)) = (
                record.result_snapshot.as_ref(),
                record.result_identity.as_ref(),
                record.prepared_remote.as_ref(),
            ) {
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

    fn mark_result_return_pending(&mut self, movement_id: &str) {
        if let Some(record) = self.workspace_movements.get_mut(movement_id) {
            if record.result_snapshot.is_some()
                && !matches!(
                    record.status.state,
                    NativeAgentWorkspaceMovementStateV1::Completed
                        | NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
                )
            {
                record.status.state = NativeAgentWorkspaceMovementStateV1::ReturningResult;
                record.status.code = Some("result_return_retry_required".into());
                let task_id = record.status.task_id.clone();
                let _ = record;
                let _ = self.persist_envelope(&task_id, None);
            }
        }
    }

    pub(crate) fn reconciliation_fact(
        &self,
        task_id: &str,
        movement_id: Option<&str>,
        executing_host_ref: &str,
    ) -> AppResult<NativeAgentReconciliationV1> {
        let task = self.task_status(task_id)?;
        let movement = match movement_id {
            Some(id) => Some(self.workspace_movements.get(id).ok_or_else(|| {
                AppError::InvalidInput("Native Agent movement is unavailable.".into())
            })?),
            None => None,
        };
        if movement.is_some_and(|value| value.status.task_id != task_id) {
            return invalid("Native Agent reconciliation crossed task correlation.");
        }
        Ok(NativeAgentReconciliationV1 {
            schema_version: NATIVE_AGENT_PROTOCOL_SCHEMA.into(),
            task_id: task_id.into(),
            movement_id: movement_id.map(str::to_owned),
            executing_host_ref: executing_host_ref.into(),
            task_state: task.state,
            movement_state: movement.map(|value| value.status.state.clone()),
            result_digest: movement.and_then(|value| {
                value
                    .result_identity
                    .as_ref()
                    .map(|identity| identity.digest.clone())
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
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks.get_mut(&fact.task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if task.state != NativeAgentTaskStateV1::Cancelled {
            task.state = fact.task_state;
            task.code = fact.code.clone();
        }
        drop(tasks);
        if let Some(movement_id) = fact.movement_id.as_ref() {
            if let Some(movement) = self.workspace_movements.get_mut(movement_id) {
                if movement.status.task_id != fact.task_id {
                    return invalid("Native Agent reconciliation crossed movement correlation.");
                }
                if !movement.apply_completed {
                    if let Some(state) = fact.movement_state {
                        movement.status.state = state;
                    }
                    movement.status.code = fact.code;
                }
            }
        }
        self.persist_envelope(&fact.task_id, None)
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
        let record = self
            .workspace_movements
            .get_mut(movement_id)
            .ok_or_else(|| {
                AppError::InvalidInput("Native Agent workspace movement is unavailable.".into())
            })?;
        if record.apply_completed
            || record.status.state == NativeAgentWorkspaceMovementStateV1::Completed
        {
            return Ok(record.status.clone());
        }
        let source = record.source.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Native Agent result return arrived at the wrong Host.".into())
        })?;
        validate_workspace_transfer_fidelity(returned_workspace)?;
        let current = crate::safe_file_identity::capture_regular_file_set_identity(
            &source.workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if current != source.baseline {
            record.status.state = NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired;
            record.status.code = Some("source_changed_since_approval".into());
            let status = record.status.clone();
            let task_id = record.status.task_id.clone();
            let _ = record;
            self.persist_envelope(&task_id, None)?;
            return Ok(status);
        }
        let returned = crate::safe_file_identity::capture_regular_file_set_identity(
            returned_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        record.status.state = NativeAgentWorkspaceMovementStateV1::ApplyingResult;
        if let Err(error) = replace_workspace_from_exact_tree(
            &source.workspace,
            &source.baseline,
            returned_workspace,
            &returned,
        ) {
            record.status.state = NativeAgentWorkspaceMovementStateV1::Interrupted;
            record.status.code = Some("result_apply_interrupted".into());
            let task_id = record.status.task_id.clone();
            let _ = record;
            let _ = self.persist_envelope(&task_id, None);
            return Err(error);
        }
        record.status.state = NativeAgentWorkspaceMovementStateV1::Completed;
        record.status.code = None;
        record.apply_completed = true;
        let status = record.status.clone();
        let task_id = record.status.task_id.clone();
        let _ = record;
        self.persist_envelope(&task_id, None)?;
        Ok(status)
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
            if self.task_workspaces.get(task_id) != Some(&workspace) {
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
            if self.task_workspaces.get(task_id) != Some(&workspace) {
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
        let workspace = workspace.to_path_buf();
        if let Some(existing) = self
            .tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
        {
            if self.task_workspaces.get(task_id) != Some(&workspace) {
                return invalid("Native Agent task identity was replayed for another workspace.");
            }
            return Ok(existing);
        }
        if !codex_usable_at(executable) {
            return invalid(
                "Codex is detected but its native app-server interface is unavailable.",
            );
        }
        if self
            .active_workspaces
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Native Agent session store is unavailable.".into())
            })?
            .contains_key(&workspace)
        {
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
            if let Ok(mut tasks) = tasks.lock() {
                let Some(status) = tasks.get_mut(&task_id) else {
                    return;
                };
                // Cancellation wins a concurrent late native completion. A
                // cancelled or indeterminate envelope must never become DONE.
                if status.state == NativeAgentTaskStateV1::Cancelled {
                    return;
                }
                match outcome {
                    Ok(()) => {
                        status.state = NativeAgentTaskStateV1::Completed;
                        status.result = Some("Codex completed its native task.".into());
                    }
                    Err(error) if error.message().contains("cancelled") => {
                        status.state = NativeAgentTaskStateV1::Cancelled;
                        status.code = Some("native_agent_cancelled".into());
                    }
                    Err(error) if error.message().contains("failed") => {
                        status.state = NativeAgentTaskStateV1::Failed;
                        status.code = Some("native_agent_failed".into());
                        status.result = Some(error.message().into());
                    }
                    Err(error) if error.message().contains("interrupted") => {
                        status.state = NativeAgentTaskStateV1::Interrupted;
                        status.code = Some("native_agent_interrupted".into());
                        status.result = Some(error.message().into());
                    }
                    Err(error) => {
                        status.state = NativeAgentTaskStateV1::Interrupted;
                        status.code = Some("native_agent_outcome_unknown".into());
                        status.result = Some(error.message().into());
                    }
                }
                if let Some(paths) = durable_paths.as_ref() {
                    NativeAgentServiceV1::persist_task_status_after_native_turn(
                        paths, &task_id, status,
                    );
                }
                active_workspaces
                    .lock()
                    .ok()
                    .map(|mut active| active.remove(&completed_workspace));
            }
        });
        Ok(status)
    }

    pub(crate) fn task_status(&self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        self.tasks
            .lock()
            .map_err(|_| AppError::InvalidInput("Native Agent task store is unavailable.".into()))?
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))
    }

    pub(crate) fn queue_remote_task(
        &mut self,
        task_id: &str,
        target_host_ref: &str,
        workspace: &str,
    ) -> AppResult<NativeAgentTaskStatusV1> {
        if task_id.trim().is_empty()
            || target_host_ref.trim().is_empty()
            || workspace.trim().is_empty()
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
                || existing.workspace_name
                    != workspace.rsplit(['/', '\\']).next().unwrap_or("workspace")
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
        self.persist_envelope(task_id, None)?;
        Ok(status)
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
        let current = tasks.get(&remote.task_id).ok_or_else(|| {
            AppError::InvalidInput("Remote native Agent task is unavailable.".into())
        })?;
        if current.state == NativeAgentTaskStateV1::Cancelled {
            return Ok(current.clone());
        }
        tasks.insert(remote.task_id.clone(), remote.status.clone());
        if let Some(record) = self
            .workspace_movements
            .values_mut()
            .find(|record| record.status.task_id == remote.task_id)
        {
            record.status.state = match remote.status.state {
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
            record.status.code = remote.status.code.clone();
        }
        drop(tasks);
        self.persist_envelope(&remote.task_id, None)?;
        Ok(remote.status)
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
        ) {
            task.state = NativeAgentTaskStateV1::Cancelled;
            task.code = Some("native_agent_cancelled".into());
        }
        let status = task.clone();
        drop(tasks);
        self.persist_envelope(task_id, None)?;
        Ok(status)
    }

    pub(crate) fn fail_remote_delivery(&mut self, task_id: &str) {
        if let Ok(mut tasks) = self.tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                if matches!(
                    task.state,
                    NativeAgentTaskStateV1::Queued | NativeAgentTaskStateV1::Running
                ) {
                    task.state = NativeAgentTaskStateV1::Failed;
                    task.code = Some("remote_agent_delivery_failed".into());
                }
            }
        }
        let _ = self.persist_envelope(task_id, None);
    }

    pub(crate) fn cancel_task(&mut self, task_id: &str) -> AppResult<NativeAgentTaskStatusV1> {
        let workspace =
            self.task_workspaces.get(task_id).cloned().ok_or_else(|| {
                AppError::InvalidInput("Native Agent task is unavailable.".into())
            })?;
        let mut tasks = self.tasks.lock().map_err(|_| {
            AppError::InvalidInput("Native Agent task store is unavailable.".into())
        })?;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| AppError::InvalidInput("Native Agent task is unavailable.".into()))?;
        if task.state != NativeAgentTaskStateV1::Running {
            return Ok(task.clone());
        }
        task.state = NativeAgentTaskStateV1::Cancelled;
        task.code = Some("native_agent_cancelled".into());
        // Set the terminal envelope before the fallible native interrupt so a
        // racing native completion can never claim DONE.
        if let Some(session) = self.codex_sessions.get(&workspace) {
            session.controller.interrupt();
        }
        self.active_workspaces
            .lock()
            .ok()
            .map(|mut active| active.retain(|_, active_task| active_task != task_id));
        let status = task.clone();
        drop(tasks);
        self.persist_envelope(task_id, None)?;
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

/// The only filesystem mutation in this slice. It stages an already-scanned
/// result beside the original workspace, rechecks the approved baseline, then
/// swaps the exact directory. If either recheck or swap fails it returns a
/// non-DONE state; it never attempts a merge.
fn replace_workspace_from_exact_tree(
    source_workspace: &Path,
    approved_baseline: &crate::safe_file_identity::RegularFileSetIdentity,
    returned_workspace: &Path,
    returned_identity: &crate::safe_file_identity::RegularFileSetIdentity,
) -> AppResult<()> {
    let observed = crate::safe_file_identity::capture_regular_file_set_identity(
        source_workspace,
        crate::storage::MAX_FILE_SIZE_BYTES,
    )?;
    if &observed != approved_baseline {
        return invalid("Native Agent source changed before result apply.");
    }
    let parent = source_workspace.parent().ok_or_else(|| {
        AppError::InvalidInput("Native Agent source workspace has no safe parent.".into())
    })?;
    let token = Uuid::new_v4().to_string();
    let stage = parent.join(format!(".pastey-agent-apply-{token}"));
    let backup = parent.join(format!(".pastey-agent-backup-{token}"));
    fs::create_dir(&stage)?;
    let staged = (|| {
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
            fs::write(destination, bytes)?;
        }
        let staged_identity = crate::safe_file_identity::capture_regular_file_set_identity(
            &stage,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if !same_logical_file_set(&staged_identity, returned_identity) {
            return invalid("Native Agent staged result changed before apply.");
        }
        let final_source = crate::safe_file_identity::capture_regular_file_set_identity(
            source_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if &final_source != approved_baseline {
            return invalid("Native Agent source changed before result apply.");
        }
        fs::rename(source_workspace, &backup)?;
        if let Err(error) = fs::rename(&stage, source_workspace) {
            let _ = fs::rename(&backup, source_workspace);
            return Err(error.into());
        }
        let applied = crate::safe_file_identity::capture_regular_file_set_identity(
            source_workspace,
            crate::storage::MAX_FILE_SIZE_BYTES,
        )?;
        if !same_logical_file_set(&applied, returned_identity) {
            return invalid("Native Agent result did not survive bounded apply.");
        }
        fs::remove_dir_all(&backup)?;
        Ok(())
    })();
    if staged.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    staged
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
    runtime
        .native_agents
        .lock()
        .validate_workspace_transfer(metadata, runtime.local_host_ref.as_str())?;
    let tree = crate::regular_file_set_transfer::materialize_package(
        &package_path,
        &runtime.paths.temp_dir,
        &metadata.content_digest,
        metadata.logical_byte_count,
    )?;
    crate::regular_file_set_transfer::cleanup_received_package(&package_path);
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
    if let Err(error) = acquisition {
        crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
        return Err(error);
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
    match outcome {
        Ok(Some(status))
            if status.state == NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired =>
        {
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
            Ok(status)
        }
        Ok(_) => runtime
            .native_agents
            .lock()
            .movement_status(&metadata.movement_id),
        Err(error) => {
            crate::regular_file_set_transfer::cleanup_materialized_tree(&tree);
            Err(error)
        }
    }
}

/// Watches the bounded native lifecycle after an outbound workspace has
/// landed. Native completion is reported first; only then does Pastey scan the
/// task tree and use the existing encrypted Transfer path for the result.
pub(crate) async fn monitor_received_workspace_task(
    runtime: Arc<crate::host_runtime::HostRuntime>,
    room_id: String,
    peer_session_id: String,
    movement_id: String,
) {
    let mut last: Option<NativeAgentTaskStatusV1> = None;
    for _ in 0..(15 * 60 * 4) {
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
                    runtime
                        .native_agents
                        .lock()
                        .mark_result_return_pending(&movement_id);
                }
                return;
            }
            NativeAgentTaskStateV1::Failed
            | NativeAgentTaskStateV1::Cancelled
            | NativeAgentTaskStateV1::Interrupted => return,
        }
    }
    runtime
        .native_agents
        .lock()
        .interrupt_workspace_movement(&movement_id, "native_agent_status_timeout");
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
    let package = crate::regular_file_set_transfer::prepare_package(
        &snapshot,
        &snapshot,
        &identity,
        &runtime.paths.temp_dir,
    )?;
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
        &package,
        Some("Codex result".into()),
        Some("application/octet-stream".into()),
    )?;
    let metadata = NativeAgentWorkspaceTransferV1 {
        schema_version: NATIVE_AGENT_WORKSPACE_MOVEMENT_SCHEMA.into(),
        movement_id: movement_id.into(),
        task_id,
        phase: NativeAgentWorkspaceTransferPhaseV1::Return,
        bridge_id: room_id.into(),
        source_host_ref: runtime.local_host_ref.as_str().into(),
        destination_host_ref: source_host.as_str().into(),
        object: crate::bridge_plan_v2::ManagedObjectRevisionV2 {
            logical_object_id: result.object.logical_object_id,
            revision: result.object.revision,
        },
        content_digest: identity.digest,
        logical_byte_count: identity.byte_count,
    };
    let sent = crate::transfer::send_native_agent_workspace_to_current_remote_session(
        runtime.clone(),
        room_id,
        &item.id,
        &package,
        session,
        metadata,
    )
    .await;
    let _ = crate::storage::delete_room_item(&runtime.paths, &item.id);
    crate::regular_file_set_transfer::cleanup_package(&package);
    sent
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
        .captured_result_snapshot_for_return(movement_id)?;
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
    {
        return invalid("Native Agent reconciliation fact is invalid.");
    }
    Ok(())
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

fn codex_usable() -> bool {
    codex_usable_at(Path::new("codex"))
}

fn codex_detected() -> bool {
    Command::new("codex")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn codex_usable_at(executable: &Path) -> bool {
    Command::new(executable)
        .args(["app-server", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

struct CodexAppServerV1 {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    messages: Mutex<mpsc::Receiver<AppResult<Value>>>,
    active_turn: Mutex<Option<(String, String)>>,
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
        };
        controller.send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "Pastey", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}}))?;
        controller.await_result(1, Instant::now() + Duration::from_secs(30))?;
        controller.send(json!({"method": "initialized", "params": {}}))?;
        Ok(controller)
    }

    fn start_thread(&self, workspace: &Path) -> AppResult<String> {
        self.send(json!({"id": 2, "method": "thread/start", "params": {"cwd": workspace}}))?;
        self.await_result(2, Instant::now() + Duration::from_secs(30))?
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a native session.".into()))
    }

    fn run_turn(&self, thread_id: &str, task: &str) -> AppResult<()> {
        let deadline = Instant::now() + MAX_TASK_WALL_TIME;
        self.send(json!({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text": task}]}}))?;
        let turn = self.await_result(3, deadline)?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AppError::InvalidInput("Codex did not return a turn id.".into()))?
            .to_owned();
        *self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))? =
            Some((thread_id.into(), turn_id.clone()));
        loop {
            let message = self.next_message(deadline)?;
            if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
                let outcome = codex_completed_turn_outcome(&message, thread_id, &turn_id);
                self.clear_active_turn()?;
                return outcome;
            }
            if message.get("error").is_some() {
                self.clear_active_turn()?;
                return invalid("Codex native task failed.");
            }
        }
    }

    fn clear_active_turn(&self) -> AppResult<()> {
        *self
            .active_turn
            .lock()
            .map_err(|_| AppError::InvalidInput("Codex session state is unavailable.".into()))? =
            None;
        Ok(())
    }

    fn interrupt(&self) {
        if let Ok(mut active) = self.active_turn.lock() {
            if let Some((thread_id, turn_id)) = active.take() {
                let _ = self.send(json!({"id": 4, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}}));
            }
        }
    }
    fn shutdown(&self) {
        self.interrupt();
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
fn codex_completed_turn_outcome(message: &Value, thread_id: &str, turn_id: &str) -> AppResult<()> {
    let params = message
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AppError::InvalidInput("Codex terminal task outcome is malformed.".into())
        })?;
    if params.get("threadId").and_then(Value::as_str) != Some(thread_id) {
        return invalid("Codex terminal task outcome crossed its native session.");
    }
    let turn = params
        .get("turn")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AppError::InvalidInput("Codex terminal task outcome is malformed.".into())
        })?;
    if turn.get("id").and_then(Value::as_str) != Some(turn_id) {
        return invalid("Codex terminal task outcome crossed its native turn.");
    }
    match turn.get("status").and_then(Value::as_str) {
        Some("completed") if turn.get("error").is_none_or(Value::is_null) => Ok(()),
        Some("completed") => invalid("Codex native task completed with an error."),
        Some("failed") => invalid("Codex native task failed."),
        Some("interrupted") => invalid("Codex native task interrupted."),
        // `cancelled` is not currently emitted by Codex's schema, but treating
        // it as an explicit non-success protects this boundary across native
        // protocol versions and lets the task envelope surface cancellation.
        Some("cancelled") => invalid("Codex native task cancelled."),
        Some("inProgress") => invalid("Codex native task terminal outcome is incomplete."),
        _ => invalid("Codex terminal task outcome is malformed."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        fixture_with_terminal(
            r#"{"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","items":[],"status":"completed","error":null}}}"#,
        )
    }

    fn fixture_with_terminal(terminal: &str) -> (PathBuf, PathBuf, PathBuf) {
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
    *'"id":3'*) echo '{{"id":3,"result":{{"turn":{{"id":"native-turn"}}}}}}'; echo '{terminal}' ;;
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

    fn wait_for_terminal(service: &NativeAgentServiceV1, task_id: &str) -> NativeAgentTaskStatusV1 {
        for _ in 0..100 {
            let status = service.task_status(task_id).unwrap();
            if status.state != NativeAgentTaskStateV1::Running {
                return status;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture native Agent did not complete")
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
        if !codex_usable() {
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
    fn remote_status_requires_the_exact_selected_host_and_cancellation_wins() {
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("request-1", "host:remote", "/remote/workspace")
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
            logical_object_id: "managed-object:v1:movement-test".into(),
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
    fn returned_workspace_applies_once_or_enters_conflict_recovery() {
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
            NativeAgentWorkspaceMovementStateV1::ConflictRecoveryRequired
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

    fn durable_paths(root: &Path) -> crate::storage::AppPaths {
        let paths = crate::storage::AppPaths::new(root.join("app-data"), root.join("logs"));
        paths.ensure_directories().unwrap();
        crate::storage::init_database(&paths).unwrap();
        paths
    }

    #[test]
    fn restart_before_native_completion_is_interrupted_not_reexecuted() {
        let root = std::env::temp_dir().join(format!("pastey-native-recovery-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .queue_remote_task("task-restart", "host:remote", "/workspace")
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
    fn durable_duplicate_and_conflicting_task_reuse_are_distinguished_after_restart() {
        let root = std::env::temp_dir().join(format!("pastey-native-replay-{}", Uuid::new_v4()));
        let paths = durable_paths(&root);
        let mut before = NativeAgentServiceV1::with_paths(paths.clone()).unwrap();
        before
            .queue_remote_task("task-replay", "host:remote", "/workspace")
            .unwrap();
        let mut after = NativeAgentServiceV1::with_paths(paths).unwrap();
        assert_eq!(
            after
                .queue_remote_task("task-replay", "host:remote", "/workspace")
                .unwrap()
                .state,
            NativeAgentTaskStateV1::Interrupted
        );
        assert!(after
            .queue_remote_task("task-replay", "host:other", "/workspace")
            .is_err());
        assert!(after
            .queue_remote_task("task-replay", "host:remote", "/other")
            .is_err());
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
    fn reconciliation_rejects_a_replaced_or_wrong_selected_host() {
        let mut service = NativeAgentServiceV1::default();
        service
            .queue_remote_task("task-reconcile", "host:current", "/workspace")
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
}

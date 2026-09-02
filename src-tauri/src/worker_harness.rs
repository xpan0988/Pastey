//! Phase 6's first, deliberately narrow Worker Harness slice.
//!
//! A Worker runs one already-claimed same-Host Transform or Execute. It receives no raw
//! paths, handles, topology, grants, or Core stores. Its alias-only resource
//! tools and optional contained-process entrypoint are pure adapters into the
//! Phase 5 `ToolRequestV1`/effect boundary; Host enforcement remains the only
//! real-effect path.

#![allow(dead_code)] // The crate-private v2 coordinator is intentionally the only live caller.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    effect_authority::{
        lower_tool_request, EffectBudgetsV1, EffectDecisionV1, EffectEvidenceV1, EffectFactsV1,
        EffectRequestKindV1, ProcessEffectV1, ResourceEffectV1, ResourceVerbV1, ResultContractV1,
        StepWorkDescriptorV1, ToolEffectIntentV1, ToolRequestV1, EFFECT_AUTHORITY_VERSION,
    },
    error::{AppError, AppResult},
    execution_world::{
        CompletedProcessObservationV1, HostManagedProcessBackendV1, ManagedProcessInvocationV1,
    },
    host_identity::HostSessionBinding,
    host_runtime::HostRuntime,
    managed_execution::{
        AuthoritativeExecuteResultV1, ExecuteResultProposalV1, ManagedObjectRevisionResultV1,
        ManagedStepClaimRequestV1, ManagedStepGrantV1, TransformResultProposalV1,
    },
    managed_resources::HostManagedResourceBackendV1,
    managed_workspace::{
        ManagedRunWorkspaceV1, WorkerWorkspaceOperationV1, WorkerWorkspaceProjectionV1,
    },
};

pub(crate) use crate::managed_workspace::WorkerWorkspaceAliasV1 as WorkerResourceAliasV1;

const WORKER_HARNESS_VERSION: &str = "pastey-worker-harness-v1";
const WORKER_RESOURCE_ADAPTER_VERSION: &str = "pastey-worker-resource-adapter-v1";
const WORKER_PROCESS_ADAPTER_VERSION: &str = "pastey-worker-process-adapter-v1";
const MAX_PROJECTED_READ_BYTES: usize = 16 * 1024;

#[cfg(test)]
pub(crate) fn projected_read_bytes_for_tests() -> u64 {
    MAX_PROJECTED_READ_BYTES as u64
}

/// Process-local cancellation state owned by `HostRuntime`. It has no grant,
/// cannot create a run, and is discarded at completion/restart.
#[derive(Clone, Debug)]
pub(crate) struct WorkerHarnessRunV1 {
    cancellation: Arc<AtomicBool>,
    bridge_id: String,
    session_binding_ref: String,
}

impl WorkerHarnessRunV1 {
    pub(crate) fn new(bridge_id: String, session_binding_ref: String) -> Self {
        Self {
            cancellation: Arc::new(AtomicBool::new(false)),
            bridge_id,
            session_binding_ref,
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub(crate) fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    pub(crate) fn session_binding_ref(&self) -> &str {
        &self.session_binding_ref
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkerRunLimitsV1 {
    pub(crate) max_turns: u32,
    pub(crate) max_provider_retries: u32,
    pub(crate) max_compactions: u32,
    pub(crate) protect_recent_turns: usize,
}

impl Default for WorkerRunLimitsV1 {
    fn default() -> Self {
        Self {
            max_turns: 16,
            max_provider_retries: 2,
            max_compactions: 1,
            protect_recent_turns: 4,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerToolSchemaV1 {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "tool", deny_unknown_fields)]
pub(crate) enum WorkerToolCallV1 {
    Inspect {
        resource: WorkerResourceAliasV1,
    },
    Read {
        resource: WorkerResourceAliasV1,
    },
    Create {
        relative_selector: String,
        content_base64: String,
    },
    Replace {
        relative_selector: String,
        content_base64: String,
    },
    ProcessSpawn {
        arguments: Vec<String>,
        #[serde(default)]
        environment: BTreeMap<String, String>,
        #[serde(default)]
        stdin_base64: Option<String>,
        #[serde(default)]
        working_directory: Option<WorkerResourceAliasV1>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum WorkerProviderResponseV1 {
    ToolCall {
        call: WorkerToolCallV1,
    },
    Final {
        output_selector: String,
        display_name: String,
        media_type: String,
    },
    FinalExecute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerProviderErrorKindV1 {
    Retryable,
    ContextOverflow,
    Cancelled,
    Interrupted,
    MalformedOutput,
    ProviderRevoked,
    Fatal,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkerProviderErrorV1 {
    pub(crate) kind: WorkerProviderErrorKindV1,
}

/// Bounded metadata normalized from a provider stream. It is diagnostic-only:
/// it neither enters Core evidence nor changes the effect envelope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderTurnMetadataV1 {
    pub(crate) finish_reason: Option<String>,
    pub(crate) input_tokens: Option<u32>,
    pub(crate) output_tokens: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderTurnV1 {
    pub(crate) response: WorkerProviderResponseV1,
    pub(crate) metadata: WorkerProviderTurnMetadataV1,
}

impl WorkerProviderTurnV1 {
    #[cfg(test)]
    pub(crate) fn scripted(response: WorkerProviderResponseV1) -> Self {
        Self {
            response,
            metadata: WorkerProviderTurnMetadataV1::default(),
        }
    }
}

/// Provider implementations only receive the model request and cooperative
/// cancellation signal. They cannot reach `HostRuntime` or an effect backend.
pub(crate) trait WorkerProviderV1 {
    fn next_turn(
        &mut self,
        request: WorkerProviderRequestV1,
        cancellation: &WorkerHarnessRunV1,
    ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerStepProjectionV1 {
    pub(crate) operation: String,
    pub(crate) semantic_intent: String,
    pub(crate) input_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "event", deny_unknown_fields)]
pub(crate) enum WorkerObservationV1 {
    Resource {
        operation: String,
        decision: String,
        generation: Option<u64>,
        content_digest: Option<String>,
        bytes: Option<u64>,
        text: Option<String>,
        truncated: bool,
    },
    Process {
        decision: String,
        state: Option<String>,
        exit_code: Option<i32>,
        stdout_digest: Option<String>,
        stdout_excerpt: Option<String>,
        stdout_truncated: bool,
        stderr_digest: Option<String>,
        stderr_excerpt: Option<String>,
        stderr_truncated: bool,
        duration_millis: Option<u64>,
        termination_requested: Option<bool>,
        network_denied: bool,
    },
    Rejected {
        code: String,
    },
    ProviderRetry {
        attempt: u32,
    },
    Compacted {
        prior_turns: usize,
        digest: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerTurnRecordV1 {
    pub(crate) response: Option<WorkerProviderResponseV1>,
    pub(crate) observation: Option<WorkerObservationV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderRequestV1 {
    pub(crate) system_instructions: String,
    pub(crate) step: WorkerStepProjectionV1,
    pub(crate) workspace: WorkerWorkspaceProjectionV1,
    pub(crate) tools: Vec<WorkerToolSchemaV1>,
    pub(crate) history: Vec<WorkerTurnRecordV1>,
}

struct WorkerRunInputV1 {
    grant: ManagedStepGrantV1,
    current_binding: HostSessionBinding,
    now: i64,
    live_revalidation: bool,
}

struct WorkerTurnAssemblerV1;

impl WorkerTurnAssemblerV1 {
    fn assemble(
        input: &WorkerRunInputV1,
        catalog: &WorkerToolCatalogV1,
        session: &WorkerSessionLogV1,
    ) -> WorkerProviderRequestV1 {
        WorkerProviderRequestV1 {
            system_instructions: format!(
                "{WORKER_HARNESS_VERSION}: {}",
                concat!(
                    "You are a Pastey Worker for one already-approved same-Host Transform or Execute step. ",
                    "Choose HOW only through the displayed resource tools. You receive bounded ",
                    "observations, not filesystem paths or Host handles. You cannot claim work, ",
                    "choose Hosts, create Transfer, change topology, grant authority, register ",
                    "lineage, use a terminal, select an executable, spawn outside the displayed ",
                    "contained process entrypoint, or use a network. Tool availability ",
                    "does not guarantee that Host enforcement will allow the requested effect."
                )
            ),
            step: WorkerStepProjectionV1 {
                operation: match input.grant.operation {
                    crate::effect_authority::ManagedSemanticOperationV1::Transform => "transform",
                    crate::effect_authority::ManagedSemanticOperationV1::Execute => "execute",
                }
                .into(),
                semantic_intent: input.grant.operation_intent.clone(),
                input_revision: input.grant.access.context.input_revisions[0].revision,
            },
            workspace: catalog.workspace_projection(),
            tools: catalog.schemas(),
            history: session.visible_turns(),
        }
    }
}

struct WorkerSessionLogV1 {
    turns: VecDeque<WorkerTurnRecordV1>,
    summaries: Vec<WorkerObservationV1>,
    compactions: u32,
}

impl WorkerSessionLogV1 {
    fn visible_turns(&self) -> Vec<WorkerTurnRecordV1> {
        let mut visible = Vec::new();
        visible.extend(
            self.summaries
                .iter()
                .cloned()
                .map(|observation| WorkerTurnRecordV1 {
                    response: None,
                    observation: Some(observation),
                }),
        );
        visible.extend(self.turns.iter().cloned());
        visible
    }

    fn push(&mut self, record: WorkerTurnRecordV1, limits: &WorkerRunLimitsV1) {
        self.turns.push_back(record);
        if self.turns.len() <= limits.protect_recent_turns
            || self.compactions >= limits.max_compactions
        {
            return;
        }
        let compact_count = self.turns.len() - limits.protect_recent_turns;
        let compacted = self.turns.drain(..compact_count).collect::<Vec<_>>();
        let digest =
            blake3::hash(&serde_json::to_vec(&compacted).expect("Worker turn serialization"))
                .to_hex()
                .to_string();
        self.summaries.push(WorkerObservationV1::Compacted {
            prior_turns: compacted.len(),
            digest,
        });
        self.compactions += 1;
    }

    fn compact_for_context_overflow(&mut self, limits: &WorkerRunLimitsV1) -> bool {
        if self.compactions >= limits.max_compactions || self.turns.is_empty() {
            return false;
        }
        let keep = limits.protect_recent_turns.min(self.turns.len());
        let compact_count = self.turns.len().saturating_sub(keep).max(1);
        let compacted = self.turns.drain(..compact_count).collect::<Vec<_>>();
        let digest = blake3::hash(
            &serde_json::to_vec(&compacted).expect("Worker context compaction serialization"),
        )
        .to_hex()
        .to_string();
        self.summaries.push(WorkerObservationV1::Compacted {
            prior_turns: compacted.len(),
            digest,
        });
        self.compactions += 1;
        true
    }
}

struct WorkerToolCatalogV1 {
    workspace: ManagedRunWorkspaceV1,
}

struct PreparedWorkerToolRequestV1 {
    tool_request: Option<ToolRequestV1>,
    staged_write: Option<StagedWorkerWriteV1>,
    process_invocation: Option<ManagedProcessInvocationV1>,
    expects_read: bool,
    output_selector: Option<String>,
    operation: &'static str,
}

struct StagedWorkerWriteV1 {
    handle_ref: crate::effect_authority::ResourceHandleRefV1,
    content_digest: String,
    bytes: Vec<u8>,
}

impl WorkerToolCatalogV1 {
    fn from_grant(grant: &ManagedStepGrantV1) -> AppResult<Self> {
        Ok(Self {
            workspace: grant.workspace.clone(),
        })
    }

    fn workspace_projection(&self) -> WorkerWorkspaceProjectionV1 {
        self.workspace.projection()
    }

    fn schemas(&self) -> Vec<WorkerToolSchemaV1> {
        let projection = self.workspace.projection();
        let mut schemas = worker_resource_schemas(&projection);
        if self.workspace.process().is_some() {
            schemas.push(worker_process_schema(&projection));
        }
        schemas
    }

    fn prepare(
        &self,
        authority: &crate::effect_authority::EffectAuthorityStateV1,
        call: WorkerToolCallV1,
    ) -> AppResult<PreparedWorkerToolRequestV1> {
        if matches!(call, WorkerToolCallV1::ProcessSpawn { .. }) {
            return self.prepare_process(authority, call);
        }
        let (tool_name, verb, handle_ref, selector, content, expects_read, operation) = match call {
            WorkerToolCallV1::Inspect { resource } => (
                "resource_inspect",
                ResourceVerbV1::Inspect,
                self.handle_for(
                    authority,
                    resource,
                    WorkerWorkspaceOperationV1::Inspect,
                    ".",
                )?,
                ".".into(),
                None,
                true,
                "inspect",
            ),
            WorkerToolCallV1::Read { resource } => (
                "resource_read",
                ResourceVerbV1::Read,
                self.handle_for(authority, resource, WorkerWorkspaceOperationV1::Read, ".")?,
                ".".into(),
                None,
                true,
                "read",
            ),
            WorkerToolCallV1::Create {
                relative_selector,
                content_base64,
            } => (
                "resource_create",
                ResourceVerbV1::Create,
                self.handle_for(
                    authority,
                    WorkerResourceAliasV1::Output,
                    WorkerWorkspaceOperationV1::Create,
                    &relative_selector,
                )?,
                relative_selector,
                Some(decode_content(&content_base64)?),
                false,
                "create",
            ),
            WorkerToolCallV1::Replace {
                relative_selector,
                content_base64,
            } => (
                "resource_replace",
                ResourceVerbV1::Replace,
                self.handle_for(
                    authority,
                    WorkerResourceAliasV1::Output,
                    WorkerWorkspaceOperationV1::Replace,
                    &relative_selector,
                )?,
                relative_selector,
                Some(decode_content(&content_base64)?),
                false,
                "replace",
            ),
            WorkerToolCallV1::ProcessSpawn { .. } => unreachable!("handled above"),
        };
        let staged_write = content.map(|bytes| StagedWorkerWriteV1 {
            handle_ref: handle_ref.clone(),
            content_digest: blake3::hash(&bytes).to_hex().to_string(),
            bytes,
        });
        let output_selector = matches!(verb, ResourceVerbV1::Create | ResourceVerbV1::Replace)
            .then(|| selector.clone());
        let requested_budget_slice = EffectBudgetsV1 {
            requests: 1,
            read_bytes: if expects_read {
                MAX_PROJECTED_READ_BYTES as u64
            } else {
                0
            },
            write_bytes: staged_write
                .as_ref()
                .map_or(0, |write| write.bytes.len() as u64),
            ..Default::default()
        };
        Ok(PreparedWorkerToolRequestV1 {
            tool_request: Some(ToolRequestV1 {
                tool_name: tool_name.into(),
                adapter_version_ref: WORKER_RESOURCE_ADAPTER_VERSION.into(),
                intents: vec![ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                        verb,
                        handle_ref,
                        relative_selector: selector,
                        value_digest: staged_write
                            .as_ref()
                            .map(|write| write.content_digest.clone()),
                    }),
                    requested_budget_slice,
                    preconditions: Vec::new(),
                }],
            }),
            staged_write,
            process_invocation: None,
            expects_read,
            output_selector,
            operation,
        })
    }

    fn prepare_process(
        &self,
        authority: &crate::effect_authority::EffectAuthorityStateV1,
        call: WorkerToolCallV1,
    ) -> AppResult<PreparedWorkerToolRequestV1> {
        let WorkerToolCallV1::ProcessSpawn {
            arguments,
            environment,
            stdin_base64,
            working_directory,
        } = call
        else {
            return invalid("Expected a contained process tool request.");
        };
        let process = self.workspace.resolve_process(authority).map_err(|_| {
            AppError::InvalidInput("Contained process authority is unavailable.".into())
        })?;
        let (working_directory_handle, working_directory_selector) = match working_directory {
            Some(alias) => (
                Some(self.handle_for(authority, alias, WorkerWorkspaceOperationV1::Read, ".")?),
                Some(".".into()),
            ),
            None => (None, None),
        };
        Ok(PreparedWorkerToolRequestV1 {
            tool_request: None,
            staged_write: None,
            process_invocation: Some(ManagedProcessInvocationV1 {
                executable_handle: process.executable_handle.clone(),
                argv: arguments,
                environment,
                stdin: stdin_base64.as_deref().map(decode_content).transpose()?,
                working_directory_handle,
                working_directory_selector,
            }),
            expects_read: false,
            output_selector: None,
            operation: "process_spawn",
        })
    }

    fn handle_for(
        &self,
        authority: &crate::effect_authority::EffectAuthorityStateV1,
        alias: WorkerResourceAliasV1,
        operation: WorkerWorkspaceOperationV1,
        relative_selector: &str,
    ) -> AppResult<crate::effect_authority::ResourceHandleRefV1> {
        self.workspace.resolve(
            authority,
            &self.workspace.projection(),
            alias,
            operation,
            relative_selector,
        )
    }
}

struct EffectDispatchBridgeV1;

impl EffectDispatchBridgeV1 {
    fn dispatch(
        runtime: &HostRuntime,
        grant: &ManagedStepGrantV1,
        prepared: PreparedWorkerToolRequestV1,
        first_sequence: u64,
        now: i64,
    ) -> AppResult<(WorkerObservationV1, EffectEvidenceV1, Option<String>)> {
        let descriptor = StepWorkDescriptorV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            context: grant.access.context.clone(),
            envelope_ref: grant.access.envelope_ref.clone(),
            run_control_ref: grant.access.run_control_ref.clone(),
            first_sequence,
        };
        let mut process_dispatch = false;
        let mut authority = runtime.effect_authority.lock();
        let mut resolver = runtime.managed_resources.lock();
        let mut objects = runtime.managed_objects.lock();
        let request = if let Some(invocation) = prepared.process_invocation.clone() {
            process_dispatch = true;
            let working_directory_handle = invocation.working_directory_handle.clone();
            let working_directory_selector = invocation.working_directory_selector.clone();
            let process = grant.process_world.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Contained process authority is unavailable.".into())
            })?;
            let (argv_digest, environment_digest, stdin_digest) = runtime
                .execution_worlds
                .stage_invocation(&grant.access, &process.world_ref, invocation)?;
            lower_tool_request(
                &descriptor,
                &process_spawn_tool_request(
                    process,
                    argv_digest,
                    environment_digest,
                    stdin_digest,
                    working_directory_handle,
                    working_directory_selector,
                ),
            )?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AppError::InvalidInput("Worker process lowering returned no request.".into())
            })?
        } else {
            lower_tool_request(
                &descriptor,
                prepared.tool_request.as_ref().ok_or_else(|| {
                    AppError::InvalidInput("Worker resource tool request is unavailable.".into())
                })?,
            )?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AppError::InvalidInput("Worker tool lowering returned no request.".into())
            })?
        };
        if let Some(write) = &prepared.staged_write {
            resolver.stage_write_payload(
                &authority,
                &grant.access,
                &write.handle_ref,
                &write.content_digest,
                write.bytes.clone(),
            )?;
        }
        let evidence = if process_dispatch {
            let mut backend = HostManagedProcessBackendV1::new(
                &runtime.execution_worlds,
                &mut resolver,
                &mut objects,
            );
            authority.enforce(&request, &grant.access.current, &mut backend)?
        } else {
            let mut backend = HostManagedResourceBackendV1::new(&mut resolver, &mut objects, now);
            authority.enforce(&request, &grant.access.current, &mut backend)?
        };
        if evidence.decision != EffectDecisionV1::Allowed {
            if let Some(write) = &prepared.staged_write {
                resolver.discard_staged_write_payload(
                    &grant.access,
                    &write.handle_ref,
                    &write.content_digest,
                )?;
            }
        }
        let read = if prepared.expects_read && evidence.decision == EffectDecisionV1::Allowed {
            resolver.take_read(&request.request_id)
        } else {
            None
        };
        if process_dispatch {
            let observation = match &evidence.facts {
                EffectFactsV1::ContainedProcess {
                    state,
                    exit_code,
                    stdout_digest,
                    stderr_digest,
                    termination_requested,
                    network_denied,
                    ..
                } if evidence.decision == EffectDecisionV1::Allowed => {
                    let completed = runtime
                        .execution_worlds
                        .take_completed_observation(&grant.access, &request.request_id)?;
                    process_observation(
                        evidence.decision,
                        Some(state.clone()),
                        *exit_code,
                        Some(stdout_digest.clone()),
                        Some(stderr_digest.clone()),
                        completed,
                        Some(*termination_requested),
                        *network_denied,
                    )
                }
                _ => {
                    process_observation(evidence.decision, None, None, None, None, None, None, true)
                }
            };
            return Ok((observation, evidence, None));
        }
        let (generation, content_digest, bytes) = match &evidence.facts {
            EffectFactsV1::Resource {
                generation,
                content_digest,
                bytes,
                ..
            } => (
                Some(*generation),
                Some(content_digest.clone()),
                Some(*bytes),
            ),
            _ => (None, None, None),
        };
        let (text, truncated) = match read {
            Some(read) => project_text(read.bytes),
            None => (None, false),
        };
        Ok((
            WorkerObservationV1::Resource {
                operation: prepared.operation.into(),
                decision: decision_name(evidence.decision).into(),
                generation,
                content_digest,
                bytes,
                text,
                truncated,
            },
            evidence,
            prepared.output_selector,
        ))
    }
}

pub(crate) struct WorkerRunControllerV1 {
    limits: WorkerRunLimitsV1,
}

pub(crate) enum WorkerRunCompletionV1 {
    Transform(crate::managed_objects::ManagedObjectAcquisition),
    Execute(AuthoritativeExecuteResultV1),
}

impl WorkerRunControllerV1 {
    pub(crate) fn new(limits: WorkerRunLimitsV1) -> AppResult<Self> {
        if limits.max_turns == 0 || limits.protect_recent_turns == 0 {
            return invalid("Worker run limits are invalid.");
        }
        Ok(Self { limits })
    }

    fn run<P: WorkerProviderV1>(
        &self,
        runtime: &HostRuntime,
        input: WorkerRunInputV1,
        cancellation: &WorkerHarnessRunV1,
        provider: &mut P,
    ) -> AppResult<WorkerRunCompletionV1> {
        let catalog = WorkerToolCatalogV1::from_grant(&input.grant)?;
        let mut session = WorkerSessionLogV1 {
            turns: VecDeque::new(),
            summaries: Vec::new(),
            compactions: 0,
        };
        let mut retries = 0;
        let mut sequence = 0;
        let mut output_writes = BTreeMap::<String, EffectEvidenceV1>::new();
        let mut last_successful_process: Option<EffectEvidenceV1> = None;
        for _ in 0..self.limits.max_turns {
            ensure_worker_active(runtime, &input, cancellation)?;
            let request = WorkerTurnAssemblerV1::assemble(&input, &catalog, &session);
            match provider.next_turn(request, cancellation) {
                Err(error)
                    if error.kind == WorkerProviderErrorKindV1::Retryable
                        && retries < self.limits.max_provider_retries =>
                {
                    retries += 1;
                    session.push(
                        WorkerTurnRecordV1 {
                            response: None,
                            observation: Some(WorkerObservationV1::ProviderRetry {
                                attempt: retries,
                            }),
                        },
                        &self.limits,
                    );
                }
                Err(error)
                    if error.kind == WorkerProviderErrorKindV1::ContextOverflow
                        && retries < self.limits.max_provider_retries
                        && session.compact_for_context_overflow(&self.limits) =>
                {
                    retries += 1;
                    session.push(
                        WorkerTurnRecordV1 {
                            response: None,
                            observation: Some(WorkerObservationV1::ProviderRetry {
                                attempt: retries,
                            }),
                        },
                        &self.limits,
                    );
                }
                Err(error)
                    if matches!(
                        error.kind,
                        WorkerProviderErrorKindV1::Cancelled
                            | WorkerProviderErrorKindV1::Interrupted
                    ) =>
                {
                    ensure_worker_active(runtime, &input, cancellation)?;
                    return invalid("Worker provider stream was interrupted.");
                }
                Err(_) => return invalid("Worker provider is unavailable."),
                Ok(turn) => {
                    retries = 0;
                    let response = turn.response;
                    match response.clone() {
                        WorkerProviderResponseV1::ToolCall { call } => {
                            ensure_worker_active(runtime, &input, cancellation)?;
                            let prepared = {
                                let authority = runtime.effect_authority.lock();
                                catalog.prepare(&authority, call)
                            };
                            match prepared {
                                Err(_) => session.push(
                                    WorkerTurnRecordV1 {
                                        response: Some(response),
                                        observation: Some(WorkerObservationV1::Rejected {
                                            code: "tool_request_invalid".into(),
                                        }),
                                    },
                                    &self.limits,
                                ),
                                Ok(prepared) => {
                                    let is_process = prepared.process_invocation.is_some();
                                    match EffectDispatchBridgeV1::dispatch(
                                        runtime,
                                        &input.grant,
                                        prepared,
                                        sequence,
                                        input.now,
                                    ) {
                                        Ok((observation, evidence, output_selector)) => {
                                            // A Host cancellation can terminate a contained world
                                            // while its spawn call is completing. Never turn that
                                            // terminal effect into another model turn or result.
                                            ensure_worker_active(runtime, &input, cancellation)?;
                                            sequence += 1;
                                            if evidence.decision == EffectDecisionV1::Allowed {
                                                if let Some(selector) = output_selector {
                                                    output_writes
                                                        .insert(selector, evidence.clone());
                                                }
                                            }
                                            if matches!(
                                                evidence.facts,
                                                EffectFactsV1::ContainedProcess { .. }
                                            ) && evidence.decision == EffectDecisionV1::Allowed
                                            {
                                                if matches!(
                                                    &evidence.facts,
                                                    EffectFactsV1::ContainedProcess { state, .. }
                                                        if !process_state_allows_next_turn(state)
                                                ) {
                                                    return invalid(
                                                        "Worker process outcome is interrupted or indeterminate.",
                                                    );
                                                }
                                                last_successful_process = Some(evidence.clone());
                                            }
                                            session.push(
                                                WorkerTurnRecordV1 {
                                                    response: Some(response),
                                                    observation: Some(observation),
                                                },
                                                &self.limits,
                                            );
                                        }
                                        Err(error) if is_process => return Err(error),
                                        Err(_) => session.push(
                                            WorkerTurnRecordV1 {
                                                response: Some(response),
                                                observation: Some(WorkerObservationV1::Rejected {
                                                    code: "effect_dispatch_rejected".into(),
                                                }),
                                            },
                                            &self.limits,
                                        ),
                                    }
                                }
                            }
                        }
                        WorkerProviderResponseV1::Final {
                            output_selector,
                            display_name,
                            media_type,
                        } => {
                            ensure_worker_active(runtime, &input, cancellation)?;
                            if input.grant.operation
                                != crate::effect_authority::ManagedSemanticOperationV1::Transform
                            {
                                return invalid("Execute Worker cannot submit a Transform result.");
                            }
                            let evidence =
                                output_writes.remove(&output_selector).ok_or_else(|| {
                                    AppError::InvalidInput(
                                        "Worker final proposal has no allowed output write.".into(),
                                    )
                                })?;
                            let seal = {
                                let authority = runtime.effect_authority.lock();
                                runtime.managed_resources.lock().seal_output_slot(
                                    &authority,
                                    &input.grant.access,
                                    input
                                        .grant
                                        .output_slot
                                        .as_ref()
                                        .expect("Transform output slot"),
                                    &output_selector,
                                    &evidence,
                                )?
                            };
                            let proposal = TransformResultProposalV1 {
                                attempt_id: input.grant.access.context.attempt_id.clone(),
                                step_id: input.grant.access.context.step_id.clone(),
                                context_ref: input.grant.access.context.context_ref()?,
                                envelope_ref: input.grant.access.envelope_ref.clone(),
                                run_control_ref: input.grant.access.run_control_ref.clone(),
                                input: input.grant.access.context.input_revisions[0].clone(),
                                output: ManagedObjectRevisionResultV1 {
                                    logical_object_id: input.grant.access.context.input_revisions
                                        [0]
                                    .logical_object_id
                                    .clone(),
                                    revision: input
                                        .grant
                                        .output_revision
                                        .expect("Transform output revision"),
                                    host_ref: runtime.local_host_ref.clone(),
                                    content_digest: seal.content_digest.clone(),
                                },
                                output_seal: seal,
                                evidence_ids: completion_evidence_ids(runtime, &input.grant)?,
                                evidence_head: completion_evidence_head(runtime, &input.grant)?,
                                display_name,
                                media_type,
                            };
                            let (binding, now) = completion_binding(runtime, &input, cancellation)?;
                            return runtime
                                .finalize_v2_transform(proposal, binding, now)
                                .map(WorkerRunCompletionV1::Transform);
                        }
                        WorkerProviderResponseV1::FinalExecute => {
                            ensure_worker_active(runtime, &input, cancellation)?;
                            if input.grant.operation
                                != crate::effect_authority::ManagedSemanticOperationV1::Execute
                            {
                                return invalid(
                                    "Transform Worker cannot submit an Execute result.",
                                );
                            }
                            let evidence = last_successful_process.as_ref().ok_or_else(|| {
                                AppError::InvalidInput(
                                    "Execute result requires an allowed contained process.".into(),
                                )
                            })?;
                            let EffectFactsV1::ContainedProcess {
                                state, exit_code, ..
                            } = &evidence.facts
                            else {
                                return invalid("Execute result process evidence was substituted.");
                            };
                            if state != "exited" || *exit_code != Some(0) {
                                return invalid(
                                    "Execute result requires a successful contained process exit.",
                                );
                            }
                            let proposal = ExecuteResultProposalV1 {
                                attempt_id: input.grant.access.context.attempt_id.clone(),
                                step_id: input.grant.access.context.step_id.clone(),
                                context_ref: input.grant.access.context.context_ref()?,
                                envelope_ref: input.grant.access.envelope_ref.clone(),
                                run_control_ref: input.grant.access.run_control_ref.clone(),
                                input: input.grant.access.context.input_revisions[0].clone(),
                                evidence_ids: completion_evidence_ids(runtime, &input.grant)?,
                                evidence_head: completion_evidence_head(runtime, &input.grant)?,
                                result_schema_ref: execute_result_schema(runtime, &input.grant)?,
                                result_digest: evidence.evidence_digest.clone(),
                                status: "completed".into(),
                            };
                            let (binding, now) = completion_binding(runtime, &input, cancellation)?;
                            return runtime
                                .finalize_v2_execute(proposal, binding, now)
                                .map(WorkerRunCompletionV1::Execute);
                        }
                    }
                }
            }
        }
        invalid("Worker run exceeded its turn budget.")
    }
}

impl HostRuntime {
    /// Phase 6's private same-Host Transform/Execute entry point. It is deliberately
    /// not registered with Tauri or Room Control; the native-v2 coordinator is
    /// the only intended live caller after it has made the Core step claim legal.
    pub(crate) fn run_v2_worker<P: WorkerProviderV1>(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        provider: &mut P,
    ) -> AppResult<WorkerRunCompletionV1> {
        self.run_v2_worker_mode(request, limits, provider, false)
    }

    fn run_v2_worker_mode<P: WorkerProviderV1>(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        provider: &mut P,
        live_revalidation: bool,
    ) -> AppResult<WorkerRunCompletionV1> {
        let controller = WorkerRunControllerV1::new(limits)?;
        let current_binding = request.current_binding.clone();
        let now = request.now;
        let grant = self.claim_v2_managed_step(request)?;
        let run_ref = grant.access.run_control_ref.clone();
        let run = self.register_worker_run(
            run_ref.clone(),
            grant.access.context.bridge_id.clone(),
            grant.access.context.session_binding_ref.clone(),
        );
        let result = controller.run(
            self,
            WorkerRunInputV1 {
                grant,
                current_binding,
                now,
                live_revalidation,
            },
            &run,
            provider,
        );
        if result.is_err() {
            let _ = self.cancel_managed_run(&run_ref);
        } else {
            // Completion has already passed the Core finalizer. Revoke the
            // now-quiescent process-local world without creating any new
            // Worker-visible effect or retaining executable/mount state.
            self.execution_worlds.terminate_run(&run_ref);
        }
        self.unregister_worker_run(&run_ref);
        result
    }

    pub(crate) fn run_live_v2_worker_with_provider_binding(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        binding: crate::worker_provider_config::ResolvedWorkerProviderBindingV1,
    ) -> AppResult<WorkerRunCompletionV1> {
        let mut provider =
            crate::worker_provider::OpenAICompatibleStreamingWorkerProviderV1::from_binding(
                binding,
            )?;
        self.run_v2_worker_mode(request, limits, &mut provider, true)
    }

    pub(crate) fn run_live_v2_worker<P: WorkerProviderV1>(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        provider: &mut P,
    ) -> AppResult<WorkerRunCompletionV1> {
        self.run_v2_worker_mode(request, limits, provider, true)
    }

    pub(crate) fn run_v2_transform_worker<P: WorkerProviderV1>(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        provider: &mut P,
    ) -> AppResult<crate::managed_objects::ManagedObjectAcquisition> {
        match self.run_v2_worker(request, limits, provider)? {
            WorkerRunCompletionV1::Transform(result) => Ok(result),
            WorkerRunCompletionV1::Execute(_) => invalid("Expected a Transform Worker completion."),
        }
    }

    pub(crate) fn run_v2_execute_worker<P: WorkerProviderV1>(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        provider: &mut P,
    ) -> AppResult<AuthoritativeExecuteResultV1> {
        match self.run_v2_worker(request, limits, provider)? {
            WorkerRunCompletionV1::Execute(result) => Ok(result),
            WorkerRunCompletionV1::Transform(_) => {
                invalid("Expected an Execute Worker completion.")
            }
        }
    }

    /// Private production-provider entrypoint. Host resolves one immutable,
    /// generation-bound binding before the claim/run begins; the Worker never
    /// receives provider configuration or credentials.
    pub(crate) fn run_v2_worker_with_provider_selection(
        &self,
        request: ManagedStepClaimRequestV1,
        limits: WorkerRunLimitsV1,
        selection: crate::worker_provider_config::WorkerProviderSelectionV1,
    ) -> AppResult<WorkerRunCompletionV1> {
        let binding = self.worker_provider_configs.resolve(&selection)?;
        let mut provider =
            crate::worker_provider::OpenAICompatibleStreamingWorkerProviderV1::from_binding(
                binding,
            )?;
        self.run_v2_worker(request, limits, &mut provider)
    }
}

fn completion_evidence_ids(
    runtime: &HostRuntime,
    grant: &ManagedStepGrantV1,
) -> AppResult<Vec<String>> {
    let completion = runtime.effect_authority.lock().completion_evidence(
        &grant.access.run_control_ref,
        &grant.access.envelope_ref,
        &grant.access.current,
    )?;
    Ok(completion
        .evidence
        .iter()
        .map(|evidence| evidence.evidence_id.as_str().to_owned())
        .collect())
}

fn completion_evidence_head(
    runtime: &HostRuntime,
    grant: &ManagedStepGrantV1,
) -> AppResult<String> {
    Ok(runtime
        .effect_authority
        .lock()
        .completion_evidence(
            &grant.access.run_control_ref,
            &grant.access.envelope_ref,
            &grant.access.current,
        )?
        .evidence_head)
}

fn execute_result_schema(runtime: &HostRuntime, grant: &ManagedStepGrantV1) -> AppResult<String> {
    let completion = runtime.effect_authority.lock().completion_evidence(
        &grant.access.run_control_ref,
        &grant.access.envelope_ref,
        &grant.access.current,
    )?;
    match completion.envelope.result_contract {
        ResultContractV1::Execute {
            result_schema_ref, ..
        } => Ok(result_schema_ref),
        _ => invalid("Execute result contract is unavailable."),
    }
}

fn schema(name: &str, description: &str, input_schema: serde_json::Value) -> WorkerToolSchemaV1 {
    WorkerToolSchemaV1 {
        name: name.into(),
        description: description.into(),
        input_schema,
    }
}

fn worker_resource_schemas(projection: &WorkerWorkspaceProjectionV1) -> Vec<WorkerToolSchemaV1> {
    let inspect_resources = projection.resources_for(WorkerWorkspaceOperationV1::Inspect);
    let read_resources = projection.resources_for(WorkerWorkspaceOperationV1::Read);
    let mut schemas = vec![
        schema(
            "resource_inspect",
            "Inspect the bounded input or output resource.",
            json!({"resource":{"enum":inspect_resources}}),
        ),
        schema(
            "resource_read",
            "Read bounded text from the input or output resource.",
            json!({"resource":{"enum":read_resources}}),
        ),
    ];
    let create_resources = projection.resources_for(WorkerWorkspaceOperationV1::Create);
    let replace_resources = projection.resources_for(WorkerWorkspaceOperationV1::Replace);
    if create_resources.contains(&WorkerResourceAliasV1::Output)
        && replace_resources.contains(&WorkerResourceAliasV1::Output)
    {
        schemas.extend([
            schema(
                "resource_create",
                "Create one output-relative resource generation from base64 content.",
                json!({"relative_selector":{"type":"string"},"content_base64":{"type":"string"}}),
            ),
            schema(
                "resource_replace",
                "Replace one output-relative resource generation from base64 content.",
                json!({"relative_selector":{"type":"string"},"content_base64":{"type":"string"}}),
            ),
        ]);
    }
    schemas
}

fn worker_process_schema(projection: &WorkerWorkspaceProjectionV1) -> WorkerToolSchemaV1 {
    let directories = projection.resources_for(WorkerWorkspaceOperationV1::Read);
    schema(
        "process_spawn",
        "Run the one Host-bound contained entrypoint with bounded arguments and explicit environment. No executable name, filesystem location, interactive session, or remote connection access is available.",
        json!({
            "arguments":{"type":"array","items":{"type":"string"}},
            "environment":{"type":"object","additionalProperties":{"type":"string"}},
            "stdin_base64":{"type":"string"},
            "working_directory":{"enum":directories}
        }),
    )
}

fn process_spawn_tool_request(
    process: &crate::managed_execution::ManagedProcessWorldGrantV1,
    argv_digest: String,
    environment_digest: String,
    stdin_digest: Option<String>,
    working_directory_handle: Option<crate::effect_authority::ResourceHandleRefV1>,
    working_directory_selector: Option<String>,
) -> ToolRequestV1 {
    ToolRequestV1 {
        tool_name: "contained_process_spawn".into(),
        adapter_version_ref: WORKER_PROCESS_ADAPTER_VERSION.into(),
        intents: vec![ToolEffectIntentV1 {
            effect: EffectRequestKindV1::Process(ProcessEffectV1::Spawn {
                world_ref: process.world_ref.clone(),
                executable_handle: process.executable_handle.clone(),
                argv_digest,
                working_directory_handle,
                working_directory_selector,
                environment_digest,
                stdin_digest,
            }),
            requested_budget_slice: EffectBudgetsV1 {
                requests: 1,
                process_spawns: 1,
                read_bytes: 32 * 1024,
                write_bytes: 1024 * 1024,
                cpu_millis: 30_000,
                memory_byte_millis: 8 * 1024 * 1024 * 30_000,
                wall_millis: 30_000,
                ..Default::default()
            },
            preconditions: Vec::new(),
        }],
    }
}

fn decode_content(value: &str) -> AppResult<Vec<u8>> {
    BASE64
        .decode(value)
        .map_err(|_| AppError::InvalidInput("Worker write content is not valid base64.".into()))
}

fn project_text(bytes: Vec<u8>) -> (Option<String>, bool) {
    let truncated = bytes.len() > MAX_PROJECTED_READ_BYTES;
    let projected = &bytes[..bytes.len().min(MAX_PROJECTED_READ_BYTES)];
    match std::str::from_utf8(projected) {
        Ok(text) => (Some(text.into()), truncated),
        Err(_) => (None, truncated),
    }
}

fn process_observation(
    decision: EffectDecisionV1,
    state: Option<String>,
    exit_code: Option<i32>,
    stdout_digest: Option<String>,
    stderr_digest: Option<String>,
    completed: Option<CompletedProcessObservationV1>,
    termination_requested: Option<bool>,
    network_denied: bool,
) -> WorkerObservationV1 {
    let (stdout_excerpt, stdout_truncated, stderr_excerpt, stderr_truncated, duration_millis) =
        completed.map_or((None, false, None, false, None), |completed| {
            (
                redact_process_excerpt(completed.stdout_excerpt),
                completed.stdout_truncated,
                redact_process_excerpt(completed.stderr_excerpt),
                completed.stderr_truncated,
                Some(completed.duration_millis),
            )
        });
    WorkerObservationV1::Process {
        decision: decision_name(decision).into(),
        state,
        exit_code,
        stdout_digest,
        stdout_excerpt,
        stdout_truncated,
        stderr_digest,
        stderr_excerpt,
        stderr_truncated,
        duration_millis,
        termination_requested,
        network_denied,
    }
}

fn redact_process_excerpt(bytes: Vec<u8>) -> Option<String> {
    let text = std::str::from_utf8(&bytes).ok()?;
    let redacted = text
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            let body = token.trim_end_matches(char::is_whitespace);
            let suffix = &token[body.len()..];
            let windows_drive_path = body.as_bytes().get(1).is_some_and(|value| *value == b':')
                && body.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
            if body.contains('/') || body.contains('\\') || windows_drive_path {
                format!("[redacted-path]{suffix}")
            } else {
                token.to_owned()
            }
        })
        .collect::<String>();
    Some(redacted)
}

fn process_state_allows_next_turn(state: &str) -> bool {
    !matches!(state, "indeterminate" | "cancelled")
}

fn decision_name(decision: EffectDecisionV1) -> &'static str {
    match decision {
        EffectDecisionV1::Allowed => "allowed",
        EffectDecisionV1::Denied => "denied",
        EffectDecisionV1::Unavailable => "unavailable",
    }
}

fn ensure_active(cancellation: &WorkerHarnessRunV1) -> AppResult<()> {
    if cancellation.is_cancelled() {
        return invalid("Worker run was cancelled.");
    }
    Ok(())
}

fn ensure_worker_active(
    runtime: &HostRuntime,
    input: &WorkerRunInputV1,
    cancellation: &WorkerHarnessRunV1,
) -> AppResult<()> {
    ensure_active(cancellation)?;
    if input.live_revalidation {
        crate::host_runtime::validate_current_host_session_binding(
            runtime,
            &input.current_binding,
            crate::storage::now_ts(),
        )?;
    }
    Ok(())
}

fn completion_binding(
    runtime: &HostRuntime,
    input: &WorkerRunInputV1,
    cancellation: &WorkerHarnessRunV1,
) -> AppResult<(HostSessionBinding, i64)> {
    ensure_worker_active(runtime, input, cancellation)?;
    if !input.live_revalidation {
        return Ok((input.current_binding.clone(), input.now));
    }
    let now = crate::storage::now_ts();
    let current = crate::host_runtime::current_host_session_binding(
        runtime,
        &input.current_binding.bridge_id,
        &input.current_binding.peer_route_ref,
    )?;
    input.current_binding.validate_current(&current, now)?;
    Ok((current, now))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_catalog_exposes_only_aliases_and_no_authority_handles() {
        let projection = WorkerWorkspaceProjectionV1::input_output_for_test(true);
        let serialized = serde_json::to_string(&worker_resource_schemas(&projection)).unwrap();
        assert!(serialized.contains("resource_read"));
        assert!(!serialized.contains("handle_ref"));
        assert!(!serialized.contains("process"));
        assert!(!serialized.contains("network"));
    }

    #[test]
    fn process_schema_has_no_executable_path_network_or_terminal_authority() {
        let projection = WorkerWorkspaceProjectionV1::input_output_for_test(true);
        let serialized = serde_json::to_string(&worker_process_schema(&projection)).unwrap();
        assert!(serialized.contains("process_spawn"));
        assert!(!serialized.contains("executable_handle"));
        assert!(!serialized.contains("path"));
        assert!(!serialized.contains("network"));
        assert!(!serialized.contains("terminal"));
    }

    #[test]
    fn process_schema_is_deterministic_and_has_no_network_effect() {
        let projection = WorkerWorkspaceProjectionV1::input_output_for_test(true);
        assert_eq!(
            serde_json::to_string(&worker_process_schema(&projection)).unwrap(),
            serde_json::to_string(&worker_process_schema(&projection)).unwrap(),
        );
    }

    #[test]
    fn process_excerpt_is_bounded_and_redacts_host_paths() {
        let excerpt = redact_process_excerpt(b"ok /Users/example/private.txt\n".to_vec()).unwrap();
        assert!(excerpt.contains("[redacted-path]"));
        assert!(!excerpt.contains("/Users/example"));
        let windows = redact_process_excerpt(
            br"ok C:\Users\example\private.txt \\server\share\private.txt".to_vec(),
        )
        .unwrap();
        assert!(!windows.contains("C:"));
        assert!(!windows.contains("\\server"));
        assert_eq!(windows.matches("[redacted-path]").count(), 2);
        assert_eq!(MAX_PROJECTED_READ_BYTES, 16 * 1024);
    }

    #[test]
    fn interrupted_or_indeterminate_processes_never_create_retry_turns() {
        assert!(process_state_allows_next_turn("failed"));
        assert!(!process_state_allows_next_turn("cancelled"));
        assert!(!process_state_allows_next_turn("indeterminate"));
    }

    #[test]
    fn compaction_keeps_complete_turn_records_and_one_summary() {
        let limits = WorkerRunLimitsV1 {
            protect_recent_turns: 2,
            max_compactions: 1,
            ..Default::default()
        };
        let mut log = WorkerSessionLogV1 {
            turns: VecDeque::new(),
            summaries: Vec::new(),
            compactions: 0,
        };
        for _ in 0..3 {
            log.push(
                WorkerTurnRecordV1 {
                    response: Some(WorkerProviderResponseV1::ToolCall {
                        call: WorkerToolCallV1::Read {
                            resource: WorkerResourceAliasV1::Input,
                        },
                    }),
                    observation: Some(WorkerObservationV1::Rejected {
                        code: "denied".into(),
                    }),
                },
                &limits,
            );
        }
        assert_eq!(log.turns.len(), 2);
        assert_eq!(log.summaries.len(), 1);
        assert!(log
            .turns
            .iter()
            .all(|turn| turn.response.is_some() && turn.observation.is_some()));
    }

    #[test]
    fn cancellation_is_observed_before_provider_or_effect_dispatch() {
        let run = WorkerHarnessRunV1::new("bridge".into(), "binding".into());
        run.cancel();
        assert!(ensure_active(&run).is_err());
    }
}

//! Core-owned managed-execution attachment for native Bridge Plan v2 steps.
//!
//! This module is intentionally not a Tauri command or protocol handler. It
//! claims one immutable, approved, admitted, dependency-eligible Transform or
//! Execute step, constructs the existing process-local EffectAuthority, and
//! accepts only Host-authenticated evidence plus a proposal-only Worker result.

#![allow(dead_code)] // Core keeps this broader than the current crate-private Worker caller.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    bridge_plan_v2::{participant_for_ref, PlanApprovalV2, PlanRevisionV2, PlanStepV2},
    effect_authority::{
        compile_effect_envelope, execution_world_ref_for, AuthorityCeilingV1,
        AuthorityContextRefV1, AuthorityContextV1, CompletionEvidenceV1, ConfinementPropertyV1,
        CurrentHostAuthorityV1, EffectBoundV1, EffectBudgetsV1, EffectCapabilityV1,
        EffectEnvelopeCompileRequestV1, EffectEnvelopeRefV1, ExecutionWorldGrantV1,
        ManagedInputRevisionV1, ManagedRunRefV1, ManagedSemanticOperationV1, NetworkAuthorityV1,
        ProcessVerbV1, ResourceGrantSpecV1, ResourceHandleRefV1, ResourceKindV1, ResourceVerbV1,
        ResultContractV1, EFFECT_AUTHORITY_VERSION,
    },
    error::{AppError, AppResult},
    host_admission::{
        HostAdmissionRequestV2, HostAdmissionService, ManagedPrimitiveAvailabilityV1,
    },
    host_identity::{HostRef, HostSessionBinding},
    host_runtime::HostRuntime,
    managed_objects::{ManagedLogicalObjectRevision, ManagedObjectAcquisition},
    managed_resources::{
        ExecutableBindingSpecV1, ManagedResourceAccessV1, ManagedResourceResolverV1,
        SealedOutputEvidenceV1,
    },
    managed_workspace::{
        ManagedRunWorkspaceV1, ManagedWorkspaceProcessBindingV1,
        ManagedWorkspaceResourceAttachmentV1, WorkerWorkspaceAliasV1,
    },
    storage::AppPaths,
};

const ATTACHMENT_VERSION: &str = "pastey-phase5-v2-attachment-v1";
const HOST_POLICY_SNAPSHOT: &str = "pastey-phase5-v2-host-policy-v1";
const NO_WORKER_WORLD_IDENTITY: &str = "pastey-phase5-v2-no-worker-world-v1";

#[derive(Clone, Debug)]
pub(crate) struct ManagedStepClaimRequestV1 {
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) input: ManagedObjectAcquisition,
    pub(crate) captured_binding: HostSessionBinding,
    pub(crate) current_binding: HostSessionBinding,
    pub(crate) now: i64,
    /// Host-private, preselected execution-world entry point. It is supplied
    /// by the future coordinator, never by a Worker/provider, and is bound
    /// into the immutable envelope before the run becomes active.
    pub(crate) process_world: Option<ManagedProcessWorldSpecV1>,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedProcessWorldSpecV1 {
    pub(crate) executable: ExecutableBindingSpecV1,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedStepGrantV1 {
    pub(crate) operation: ManagedSemanticOperationV1,
    pub(crate) access: ManagedResourceAccessV1,
    pub(crate) input_handle: ResourceHandleRefV1,
    pub(crate) output_slot: Option<ResourceHandleRefV1>,
    /// Model-visible semantic projection for the one Transform step. This is
    /// not a Plan, grant, path, or Host-selection capability.
    pub(crate) transform_intent: Option<String>,
    /// One exact semantic intent projection for the already claimed primitive.
    pub(crate) operation_intent: String,
    pub(crate) output_revision: Option<u64>,
    /// Opaque Harness capability projection. The physical executable and all
    /// mounts remain Host-private in the already provisioned world.
    pub(crate) process_world: Option<ManagedProcessWorldGrantV1>,
    /// Host-local run workspace derived from the already installed envelope.
    /// It aggregates existing attachments and exposes a non-authoritative
    /// alias projection to the Harness; it is not an independent grant.
    pub(crate) workspace: ManagedRunWorkspaceV1,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedProcessWorldGrantV1 {
    pub(crate) world_ref: crate::effect_authority::ExecutionWorldRefV1,
    pub(crate) executable_handle: ResourceHandleRefV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransformResultProposalV1 {
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) input: ManagedInputRevisionV1,
    pub(crate) output: ManagedObjectRevisionResultV1,
    pub(crate) output_seal: SealedOutputEvidenceV1,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) evidence_head: String,
    pub(crate) display_name: String,
    pub(crate) media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedObjectRevisionResultV1 {
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
    pub(crate) host_ref: HostRef,
    pub(crate) content_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecuteResultProposalV1 {
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) input: ManagedInputRevisionV1,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) evidence_head: String,
    pub(crate) result_schema_ref: String,
    pub(crate) result_digest: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthoritativeExecuteResultV1 {
    pub(crate) result_ref: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) input: ManagedInputRevisionV1,
    pub(crate) host_ref: HostRef,
    pub(crate) result_schema_ref: String,
    pub(crate) result_digest: String,
    pub(crate) status: String,
    pub(crate) evidence_head: String,
}

#[derive(Clone)]
struct ClaimSourceV1 {
    revision: PlanRevisionV2,
    approval: PlanApprovalV2,
    step: PlanStepV2,
    admission_ref: String,
    participant_ref: crate::host_identity::PlanParticipantRef,
    expires_at: i64,
}

impl HostRuntime {
    /// Claims one exact managed v2 step. This crate-private method has no
    /// invoke registration; Phase 6 may attach a Worker only through it.
    pub(crate) fn claim_v2_managed_step(
        &self,
        request: ManagedStepClaimRequestV1,
    ) -> AppResult<ManagedStepGrantV1> {
        let source = load_claim_source(
            &self.paths,
            &self.host_admission,
            &self.local_host_ref,
            &request,
        )?;
        let (operation, input, output_revision) = managed_step_contract(&source.step)?;
        if request.input.object.logical_object_id != input.logical_object_id
            || request.input.object.revision != input.revision
            || request.input.object.host_ref != self.local_host_ref
        {
            return invalid("Managed step input binding does not match the immutable v2 step.");
        }

        let mut objects = self.managed_objects.lock();
        let artifact = objects.resolve(&request.input, request.now)?;
        let context = AuthorityContextV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            bridge_id: source.revision.bridge_id.clone(),
            plan_id: source.revision.plan_id.clone(),
            revision_id: source.revision.revision_id.clone(),
            revision_hash: source.revision.revision_hash.clone(),
            approval_id: source.approval.approval_id.clone(),
            attempt_id: request.attempt_id.clone(),
            step_id: request.step_id.clone(),
            semantic_operation: operation,
            participant_ref: source.participant_ref.clone(),
            host_ref: self.local_host_ref.clone(),
            admission_ref: source.admission_ref.clone(),
            session_binding_ref: request.captured_binding.binding_ref.clone(),
            input_revisions: vec![ManagedInputRevisionV1 {
                logical_object_id: input.logical_object_id.clone(),
                revision: input.revision,
                host_ref: self.local_host_ref.clone(),
            }],
            issued_at: request.now,
            expires_at: source.expires_at,
        };
        let current = current_authority(&request.current_binding, request.now);
        let mut authority = self.effect_authority.lock();
        let draft = authority.begin_run(context.clone())?;
        let budgets = step_budgets();
        let process_spec = request.process_world.as_ref();
        let process_availability =
            process_spec.map(|_| self.execution_worlds.platform_availability());
        if process_availability
            .as_ref()
            .is_some_and(|availability| !availability.available)
        {
            let _ = authority.revoke_run(&draft.run_control_ref);
            return Err(AppError::InvalidInput(
                "Required platform execution world is unavailable.".into(),
            ));
        }
        let input_grant = authority.mint_resource_grant(
            &draft,
            ResourceGrantSpecV1 {
                host_ref: self.local_host_ref.clone(),
                kind: ResourceKindV1::ManagedRevision,
                safe_identity_ref: ManagedResourceResolverV1::managed_revision_identity_ref(
                    &request.input,
                    &artifact,
                )?,
                selector_prefix: ".".into(),
                allowed_verbs: [ResourceVerbV1::Inspect, ResourceVerbV1::Read]
                    .into_iter()
                    .collect(),
                budgets,
                expires_at: source.expires_at,
            },
        )?;
        let output_grant = if operation == ManagedSemanticOperationV1::Transform {
            Some(
                authority.mint_resource_grant(
                    &draft,
                    ResourceGrantSpecV1 {
                        host_ref: self.local_host_ref.clone(),
                        kind: ResourceKindV1::OutputSlot,
                        safe_identity_ref: domain_hash(
                            "pastey-phase5-v2-output-slot-v1",
                            &(draft.context_ref.as_str(), draft.run_control_ref.as_str()),
                        )?,
                        selector_prefix: ".".into(),
                        allowed_verbs: [
                            ResourceVerbV1::Inspect,
                            ResourceVerbV1::Read,
                            ResourceVerbV1::Create,
                            ResourceVerbV1::Replace,
                        ]
                        .into_iter()
                        .collect(),
                        budgets,
                        expires_at: source.expires_at,
                    },
                )?,
            )
        } else {
            None
        };
        let scratch_grant = if process_spec.is_some() {
            Some(
                authority.mint_resource_grant(
                    &draft,
                    ResourceGrantSpecV1 {
                        host_ref: self.local_host_ref.clone(),
                        kind: ResourceKindV1::Scratch,
                        safe_identity_ref: domain_hash(
                            "pastey-phase5-v2-process-scratch-v1",
                            &(draft.context_ref.as_str(), draft.run_control_ref.as_str()),
                        )?,
                        selector_prefix: ".".into(),
                        allowed_verbs: [
                            ResourceVerbV1::Inspect,
                            ResourceVerbV1::Read,
                            ResourceVerbV1::Create,
                            ResourceVerbV1::Replace,
                        ]
                        .into_iter()
                        .collect(),
                        budgets,
                        expires_at: source.expires_at,
                    },
                )?,
            )
        } else {
            None
        };
        let executable_grant = if let Some(spec) = process_spec {
            Some(
                authority.mint_resource_grant(
                    &draft,
                    ResourceGrantSpecV1 {
                        host_ref: self.local_host_ref.clone(),
                        kind: ResourceKindV1::Executable,
                        safe_identity_ref: ManagedResourceResolverV1::executable_identity_ref(
                            &spec.executable,
                        )?,
                        selector_prefix: ".".into(),
                        allowed_verbs: [ResourceVerbV1::Inspect, ResourceVerbV1::Read]
                            .into_iter()
                            .collect(),
                        budgets,
                        expires_at: source.expires_at,
                    },
                )?,
            )
        } else {
            None
        };
        let mut resources = vec![input_grant.clone()];
        if let Some(grant) = &output_grant {
            resources.push(grant.clone());
        }
        if let Some(grant) = &scratch_grant {
            resources.push(grant.clone());
        }
        if let Some(grant) = &executable_grant {
            resources.push(grant.clone());
        }
        let world_identity = process_availability
            .as_ref()
            .map(|availability| availability.identity_digest.as_str())
            .unwrap_or(NO_WORKER_WORLD_IDENTITY);
        let world_ref = execution_world_ref_for(&draft, world_identity)?;
        let world = ExecutionWorldGrantV1 {
            world_ref,
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            world_identity_digest: world_identity.into(),
            mounted_resources: resources
                .iter()
                .filter(|grant| grant.kind != ResourceKindV1::Executable)
                .map(|grant| grant.handle_ref.clone())
                .collect(),
            executable_resources: executable_grant
                .iter()
                .map(|grant| grant.handle_ref.clone())
                .collect(),
            required_properties: all_confinement_properties(),
            budgets,
            expires_at: source.expires_at,
        };
        let mut bounds = resource_bounds();
        if process_spec.is_some() {
            bounds.extend(process_bounds());
        }
        let base = AuthorityCeilingV1 {
            context_ref: draft.context_ref.clone(),
            source_snapshot_ref: "phase5-v2-semantic-ceiling-v1".into(),
            resources: resources.clone(),
            world,
            effect_bounds: bounds,
            budgets,
            network: NetworkAuthorityV1::Denied,
            expires_at: source.expires_at,
        };
        let mut admission_ceiling = base.clone();
        admission_ceiling.source_snapshot_ref = source.admission_ref.clone();
        let mut host_ceiling = base.clone();
        host_ceiling.source_snapshot_ref = HOST_POLICY_SNAPSHOT.into();
        let mut confinement_ceiling = base.clone();
        confinement_ceiling.source_snapshot_ref = world_identity.into();
        let result_contract = match operation {
            ManagedSemanticOperationV1::Transform => ResultContractV1::Transform {
                input: context.input_revisions[0].clone(),
                output_revision: output_revision.expect("Transform output"),
                output_slot: output_grant
                    .as_ref()
                    .expect("Transform output grant")
                    .handle_ref
                    .clone(),
            },
            ManagedSemanticOperationV1::Execute => ResultContractV1::Execute {
                inputs: context.input_revisions.clone(),
                result_schema_ref: execute_schema_ref(&source.step)?,
            },
        };
        let envelope = compile_effect_envelope(EffectEnvelopeCompileRequestV1 {
            context: context.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            semantic_ceiling: base,
            admission_ceiling,
            host_policy_ceiling: host_ceiling,
            confinement_ceiling,
            host_policy_snapshot_ref: HOST_POLICY_SNAPSHOT.into(),
            result_contract,
        })?;
        authority.install_envelope(draft, envelope.clone())?;
        authority.activate_run(&envelope.run_control_ref, request.now)?;
        let access = ManagedResourceAccessV1 {
            envelope_ref: envelope.envelope_ref.clone(),
            run_control_ref: envelope.run_control_ref.clone(),
            context,
            current,
        };
        let mut resolver = self.managed_resources.lock();
        if let Err(error) = resolver.bind_managed_revision(
            &authority,
            &mut objects,
            &access,
            &input_grant.handle_ref,
            request.input,
        ) {
            let _ = authority.revoke_run(&envelope.run_control_ref);
            return Err(error);
        }
        if let Some(grant) = &output_grant {
            if let Err(error) = resolver.provision_output_slot(
                &authority,
                &access,
                &grant.handle_ref,
                budgets.write_bytes,
            ) {
                resolver.purge_run(&envelope.run_control_ref);
                let _ = authority.revoke_run(&envelope.run_control_ref);
                return Err(error);
            }
        }
        if let Some(grant) = &scratch_grant {
            if let Err(error) = resolver.provision_scratch(
                &authority,
                &access,
                &grant.handle_ref,
                budgets.write_bytes,
            ) {
                resolver.purge_run(&envelope.run_control_ref);
                let _ = authority.revoke_run(&envelope.run_control_ref);
                return Err(error);
            }
        }
        if let (Some(grant), Some(spec)) = (&executable_grant, process_spec) {
            if let Err(error) = resolver.bind_executable(
                &authority,
                &access,
                &grant.handle_ref,
                spec.executable.clone(),
            ) {
                resolver.purge_run(&envelope.run_control_ref);
                let _ = authority.revoke_run(&envelope.run_control_ref);
                return Err(error);
            }
        }
        let process_world = executable_grant
            .as_ref()
            .map(|grant| ManagedProcessWorldGrantV1 {
                world_ref: envelope.world.world_ref.clone(),
                executable_handle: grant.handle_ref.clone(),
            });
        let mut workspace_attachments = vec![ManagedWorkspaceResourceAttachmentV1 {
            alias: WorkerWorkspaceAliasV1::Input,
            kind: ResourceKindV1::ManagedRevision,
            handle_ref: input_grant.handle_ref.clone(),
        }];
        if let Some(output) = &output_grant {
            workspace_attachments.push(ManagedWorkspaceResourceAttachmentV1 {
                alias: WorkerWorkspaceAliasV1::Output,
                kind: ResourceKindV1::OutputSlot,
                handle_ref: output.handle_ref.clone(),
            });
        }
        if let Some(scratch) = &scratch_grant {
            workspace_attachments.push(ManagedWorkspaceResourceAttachmentV1 {
                alias: WorkerWorkspaceAliasV1::Scratch,
                kind: ResourceKindV1::Scratch,
                handle_ref: scratch.handle_ref.clone(),
            });
        }
        let workspace = match ManagedRunWorkspaceV1::derive(
            &authority,
            access.clone(),
            workspace_attachments,
            process_world
                .as_ref()
                .map(|process| ManagedWorkspaceProcessBindingV1 {
                    world_ref: process.world_ref.clone(),
                    executable_handle: process.executable_handle.clone(),
                }),
        ) {
            Ok(workspace) => workspace,
            Err(error) => {
                resolver.purge_run(&envelope.run_control_ref);
                let _ = authority.revoke_run(&envelope.run_control_ref);
                return Err(error);
            }
        };
        if process_spec.is_some() {
            if let Err(error) = self.execution_worlds.provision_world(
                &authority,
                &mut resolver,
                &mut objects,
                access.clone(),
                &envelope.world.world_ref,
            ) {
                resolver.purge_run(&envelope.run_control_ref);
                let _ = authority.revoke_run(&envelope.run_control_ref);
                return Err(error);
            }
        }
        drop(resolver);
        drop(objects);
        if let Err(error) = insert_claim(&self.paths, &access, operation, request.now) {
            self.execution_worlds
                .terminate_run(&envelope.run_control_ref);
            self.managed_resources
                .lock()
                .purge_run(&envelope.run_control_ref);
            let _ = authority.revoke_run(&envelope.run_control_ref);
            return Err(error);
        }
        let transform_intent = match &source.step {
            PlanStepV2::Transform {
                modification_intent,
                ..
            } => Some(modification_intent.clone()),
            _ => None,
        };
        let operation_intent = match &source.step {
            PlanStepV2::Transform {
                modification_intent,
                ..
            } => modification_intent.clone(),
            PlanStepV2::Execute {
                execution_intent, ..
            } => execution_intent.clone(),
            _ => unreachable!("managed claim validated primitive"),
        };
        Ok(ManagedStepGrantV1 {
            operation,
            access,
            input_handle: input_grant.handle_ref,
            output_slot: output_grant.map(|grant| grant.handle_ref),
            transform_intent,
            operation_intent,
            output_revision,
            process_world,
            workspace,
        })
    }

    pub(crate) fn finalize_v2_transform(
        &self,
        proposal: TransformResultProposalV1,
        current_binding: HostSessionBinding,
        now: i64,
    ) -> AppResult<ManagedObjectAcquisition> {
        let _completion_guard = self.managed_completion_lock.lock();
        let source = load_completion_source(
            &self.paths,
            &proposal.attempt_id,
            &proposal.step_id,
            &proposal.context_ref,
            &proposal.envelope_ref,
            &proposal.run_control_ref,
            &current_binding,
            &self.local_host_ref,
            now,
        )?;
        let (operation, input, output_revision) = managed_step_contract(&source.step)?;
        if operation != ManagedSemanticOperationV1::Transform {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Execute authority cannot finalize Transform lineage.",
            );
        }
        let current = current_authority(&current_binding, now);
        let mut authority = self.effect_authority.lock();
        let completion = authority.completion_evidence(
            &proposal.run_control_ref,
            &proposal.envelope_ref,
            &current,
        )?;
        validate_proposal_evidence(&completion, &proposal.evidence_ids, &proposal.evidence_head)?;
        let expected_input = ManagedInputRevisionV1 {
            logical_object_id: input.logical_object_id.clone(),
            revision: input.revision,
            host_ref: self.local_host_ref.clone(),
        };
        let expected_output = output_revision.expect("Transform output revision");
        if proposal.input != expected_input
            || proposal.output.logical_object_id != input.logical_object_id
            || proposal.output.revision != expected_output
            || proposal.output.host_ref != self.local_host_ref
            || proposal.output.content_digest != proposal.output_seal.content_digest
            || proposal.output_seal.envelope_ref != proposal.envelope_ref
            || proposal.output_seal.run_control_ref != proposal.run_control_ref
            || proposal.output_seal.context_ref != proposal.context_ref
        {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Transform proposal changed input, Host, output identity, or N+1 lineage.",
            );
        }
        match &completion.envelope.result_contract {
            ResultContractV1::Transform {
                input,
                output_revision,
                output_slot,
            } if input == &expected_input
                && *output_revision == expected_output
                && output_slot == &proposal.output_seal.handle_ref => {}
            _ => {
                return fail_claim(
                    &self.paths,
                    &proposal.attempt_id,
                    &proposal.step_id,
                    "Transform result contract was substituted.",
                )
            }
        }
        if !self
            .execution_worlds
            .run_is_quiescent(&proposal.run_control_ref)
            || !self
                .network_broker
                .run_is_quiescent(&proposal.run_control_ref)
        {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Managed process or network authority is not quiescent.",
            );
        }
        let access = ManagedResourceAccessV1 {
            envelope_ref: proposal.envelope_ref.clone(),
            run_control_ref: proposal.run_control_ref.clone(),
            context: completion.envelope.context.clone(),
            current,
        };
        let mut objects = self.managed_objects.lock();
        let acquisition = self
            .managed_resources
            .lock()
            .register_sealed_transform_output(
                &mut objects,
                &access,
                &proposal.output_seal,
                input.logical_object_id.clone(),
                expected_output,
                proposal.display_name.clone(),
                proposal.media_type.clone(),
                source.expires_at,
            )
            .map_err(|_| {
                mark_claim_failed(&self.paths, &proposal.attempt_id, &proposal.step_id);
                AppError::InvalidInput(
                    "Transform OutputSlot safe identity was stale or substituted.".into(),
                )
            })?;
        authority.complete_run_authoritatively(&proposal.run_control_ref)?;
        persist_transform_result(&self.paths, &proposal, &acquisition.object, now)?;
        drop(objects);
        Ok(acquisition)
    }

    pub(crate) fn finalize_v2_execute(
        &self,
        proposal: ExecuteResultProposalV1,
        current_binding: HostSessionBinding,
        now: i64,
    ) -> AppResult<AuthoritativeExecuteResultV1> {
        let _completion_guard = self.managed_completion_lock.lock();
        let source = load_completion_source(
            &self.paths,
            &proposal.attempt_id,
            &proposal.step_id,
            &proposal.context_ref,
            &proposal.envelope_ref,
            &proposal.run_control_ref,
            &current_binding,
            &self.local_host_ref,
            now,
        )?;
        let (operation, input, _) = managed_step_contract(&source.step)?;
        if operation != ManagedSemanticOperationV1::Execute {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Transform authority cannot finalize an Execute result.",
            );
        }
        let current = current_authority(&current_binding, now);
        let mut authority = self.effect_authority.lock();
        let completion = authority.completion_evidence(
            &proposal.run_control_ref,
            &proposal.envelope_ref,
            &current,
        )?;
        validate_proposal_evidence(&completion, &proposal.evidence_ids, &proposal.evidence_head)?;
        let expected_input = ManagedInputRevisionV1 {
            logical_object_id: input.logical_object_id.clone(),
            revision: input.revision,
            host_ref: self.local_host_ref.clone(),
        };
        let expected_schema = execute_schema_ref(&source.step)?;
        if proposal.input != expected_input
            || proposal.result_schema_ref != expected_schema
            || proposal.result_digest.trim().is_empty()
            || proposal.status.trim().is_empty()
        {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Execute proposal changed its exact input or result contract.",
            );
        }
        match &completion.envelope.result_contract {
            ResultContractV1::Execute {
                inputs,
                result_schema_ref,
            } if inputs == &vec![expected_input.clone()]
                && result_schema_ref == &expected_schema => {}
            _ => {
                return fail_claim(
                    &self.paths,
                    &proposal.attempt_id,
                    &proposal.step_id,
                    "Execute result contract was substituted.",
                )
            }
        }
        if !self
            .execution_worlds
            .run_is_quiescent(&proposal.run_control_ref)
            || !self
                .network_broker
                .run_is_quiescent(&proposal.run_control_ref)
        {
            return fail_claim(
                &self.paths,
                &proposal.attempt_id,
                &proposal.step_id,
                "Managed process or network authority is not quiescent.",
            );
        }
        let result = AuthoritativeExecuteResultV1 {
            result_ref: domain_hash(
                "pastey-authoritative-execute-result-v1",
                &(
                    proposal.context_ref.as_str(),
                    proposal.evidence_head.as_str(),
                    proposal.result_digest.as_str(),
                ),
            )?,
            attempt_id: proposal.attempt_id.clone(),
            step_id: proposal.step_id.clone(),
            input: expected_input,
            host_ref: self.local_host_ref.clone(),
            result_schema_ref: proposal.result_schema_ref.clone(),
            result_digest: proposal.result_digest.clone(),
            status: proposal.status.clone(),
            evidence_head: proposal.evidence_head.clone(),
        };
        authority.complete_run_authoritatively(&proposal.run_control_ref)?;
        persist_execute_result(&self.paths, &proposal, &result, now)?;
        Ok(result)
    }
}

fn load_claim_source(
    paths: &AppPaths,
    admission_service: &HostAdmissionService,
    local_host: &HostRef,
    request: &ManagedStepClaimRequestV1,
) -> AppResult<ClaimSourceV1> {
    request
        .captured_binding
        .validate_current(&request.current_binding, request.now)?;
    let conn = connection(paths)?;
    let row = conn
        .query_row(
            "SELECT attempts.bridge_id, attempts.approval_id, attempts.plan_id,
                attempts.revision_id, attempts.revision_hash,
                attempts.target_participant_ref, attempts.correlation_id,
                attempts.session_binding_ref, attempts.admission_ref,
                attempts.expires_at, attempts.state, approvals.state,
                approvals.approval_json, revisions.revision_json
         FROM bridge_plan_v2_attempts AS attempts
         JOIN bridge_plan_v2_approvals AS approvals ON approvals.approval_id = attempts.approval_id
         JOIN bridge_plan_v2_revisions AS revisions ON revisions.revision_id = attempts.revision_id
         WHERE attempts.attempt_id = ?1",
            [&request.attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::InvalidInput("Managed v2 attempt is unavailable.".into()))?;
    let approval: PlanApprovalV2 = serde_json::from_str(&row.12)?;
    let revision: PlanRevisionV2 = serde_json::from_str(&row.13)?;
    if row.10 != "accepted"
        || row.11 != "valid"
        || row.7 != request.captured_binding.binding_ref
        || row.0 != request.captured_binding.bridge_id
        || row.9 <= request.now
        || request.current_binding.local_host_ref != *local_host
        || revision.plan_id != row.2
        || revision.revision_id != row.3
        || revision.revision_hash != row.4
        || approval.approval_id != row.1
    {
        return invalid("Managed v2 attempt authority is stale or mismatched.");
    }
    let participant_ref = revision
        .participants
        .as_slice()
        .iter()
        .find(|participant| participant.participant_ref.as_str() == row.5)
        .map(|participant| participant.participant_ref.clone())
        .ok_or_else(|| AppError::InvalidInput("Managed v2 participant is unavailable.".into()))?;
    if participant_for_ref(&revision, &participant_ref).map(|p| &p.host_ref) != Some(local_host) {
        return invalid("Managed v2 participant or Host was substituted.");
    }
    let admission_request = HostAdmissionRequestV2 {
        attempt_id: request.attempt_id.clone(),
        approval_id: approval.approval_id.clone(),
        plan_id: revision.plan_id.clone(),
        revision_id: revision.revision_id.clone(),
        revision_hash: revision.revision_hash.clone(),
        host_ref: local_host.clone(),
        participant_ref: participant_ref.clone(),
        protocol_correlation_id: row.6,
        session_binding: request.captured_binding.clone(),
    };
    let admission = admission_service.evaluate_v2_with_availability(
        &revision,
        &approval,
        &admission_request,
        &request.current_binding,
        ManagedPrimitiveAvailabilityV1::verified_attachment(local_host.clone(), true, true),
        request.now,
    )?;
    if admission
        .admitted()
        .map(|value| value.admission_ref.as_str())
        != Some(row.8.as_str())
    {
        return invalid("Managed v2 Host admission correlation is stale or mismatched.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == request.step_id)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Managed v2 step is unavailable.".into()))?;
    if !step.binds_participant(&participant_ref)
        || !matches!(
            step,
            PlanStepV2::Transform { .. } | PlanStepV2::Execute { .. }
        )
    {
        return invalid("Managed v2 step is not exact Host-bound managed work.");
    }
    let coordinated: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_v2_receiver_attempts WHERE attempt_id = ?1)",
        [&request.attempt_id],
        |row| row.get(0),
    )?;
    for dependency in step.dependencies() {
        let complete = if coordinated == 1 {
            crate::native_v2_orchestration::committed_step(&conn, &request.attempt_id, dependency)?
        } else {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM bridge_plan_v2_managed_step_claims WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'completed')",
                params![request.attempt_id, dependency],
                |row| row.get::<_, i64>(0),
            )? == 1
        };
        if !complete {
            return invalid("Managed v2 step dependencies are not authoritatively complete.");
        }
    }
    Ok(ClaimSourceV1 {
        revision,
        approval,
        step,
        admission_ref: row.8,
        participant_ref,
        expires_at: row.9.min(request.captured_binding.expires_at),
    })
}

#[allow(clippy::too_many_arguments)]
fn load_completion_source(
    paths: &AppPaths,
    attempt_id: &str,
    step_id: &str,
    context_ref: &AuthorityContextRefV1,
    envelope_ref: &EffectEnvelopeRefV1,
    run_ref: &ManagedRunRefV1,
    current_binding: &HostSessionBinding,
    local_host: &HostRef,
    now: i64,
) -> AppResult<ClaimSourceV1> {
    let conn = connection(paths)?;
    let row = conn.query_row(
        "SELECT attempts.approval_id, attempts.target_participant_ref,
                attempts.admission_ref, attempts.expires_at, attempts.state,
                approvals.state, approvals.approval_json, revisions.revision_json,
                claims.state
         FROM bridge_plan_v2_managed_step_claims AS claims
         JOIN bridge_plan_v2_attempts AS attempts ON attempts.attempt_id = claims.attempt_id
         JOIN bridge_plan_v2_approvals AS approvals ON approvals.approval_id = attempts.approval_id
         JOIN bridge_plan_v2_revisions AS revisions ON revisions.revision_id = attempts.revision_id
         WHERE claims.attempt_id = ?1 AND claims.step_id = ?2
           AND claims.context_ref = ?3 AND claims.envelope_ref = ?4 AND claims.run_control_ref = ?5",
        params![attempt_id, step_id, context_ref.as_str(), envelope_ref.as_str(), run_ref.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?,
            row.get::<_, String>(6)?, row.get::<_, String>(7)?, row.get::<_, String>(8)?)),
    ).optional()?.ok_or_else(|| AppError::InvalidInput("Exact managed v2 step claim is unavailable.".into()))?;
    let approval: PlanApprovalV2 = serde_json::from_str(&row.6)?;
    let revision: PlanRevisionV2 = serde_json::from_str(&row.7)?;
    let participant_ref = revision
        .participants
        .as_slice()
        .iter()
        .find(|participant| participant.participant_ref.as_str() == row.1)
        .map(|participant| participant.participant_ref.clone())
        .ok_or_else(|| AppError::InvalidInput("Managed v2 participant is unavailable.".into()))?;
    if row.4 != "accepted"
        || row.5 != "valid"
        || row.8 != "claimed"
        || row.3 <= now
        || current_binding.expires_at <= now
        || current_binding.local_host_ref != *local_host
        || current_binding.bridge_id != revision.bridge_id
        || participant_for_ref(&revision, &participant_ref).map(|p| &p.host_ref) != Some(local_host)
    {
        return invalid("Managed v2 completion authority is stale, expired, or substituted.");
    }
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == step_id)
        .cloned()
        .ok_or_else(|| {
            AppError::InvalidInput("Managed v2 completion step is unavailable.".into())
        })?;
    Ok(ClaimSourceV1 {
        revision,
        approval,
        step,
        admission_ref: row.2,
        participant_ref,
        expires_at: row.3.min(current_binding.expires_at),
    })
}

fn insert_claim(
    paths: &AppPaths,
    access: &ManagedResourceAccessV1,
    operation: ManagedSemanticOperationV1,
    now: i64,
) -> AppResult<()> {
    connection(paths)?.execute(
        "INSERT INTO bridge_plan_v2_managed_step_claims
         (attempt_id, step_id, operation, context_ref, envelope_ref, run_control_ref, state, claimed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'claimed', ?7)",
        params![access.context.attempt_id, access.context.step_id,
            match operation { ManagedSemanticOperationV1::Transform => "transform", ManagedSemanticOperationV1::Execute => "execute" },
            access.context.context_ref()?.as_str(), access.envelope_ref.as_str(),
            access.run_control_ref.as_str(), now],
    )?;
    Ok(())
}

fn persist_transform_result(
    paths: &AppPaths,
    proposal: &TransformResultProposalV1,
    object: &ManagedLogicalObjectRevision,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO bridge_plan_v2_transform_results
         (attempt_id, step_id, logical_object_id, input_revision, output_revision,
          host_ref, content_digest, seal_ref, evidence_head, result_json, completed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            proposal.attempt_id,
            proposal.step_id,
            object.logical_object_id,
            proposal.input.revision,
            object.revision,
            object.host_ref.as_str(),
            proposal.output.content_digest,
            proposal.output_seal.seal_ref,
            proposal.evidence_head,
            serde_json::to_string(proposal)?,
            now
        ],
    )?;
    let changed = tx.execute(
        "UPDATE bridge_plan_v2_managed_step_claims SET state = 'completed', evidence_head = ?3, completed_at = ?4 WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'claimed'",
        params![proposal.attempt_id, proposal.step_id, proposal.evidence_head, now],
    )?;
    if changed != 1 {
        return invalid("Transform completion lost its exact one-use claim.");
    }
    tx.commit()?;
    Ok(())
}

fn persist_execute_result(
    paths: &AppPaths,
    proposal: &ExecuteResultProposalV1,
    result: &AuthoritativeExecuteResultV1,
    now: i64,
) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO bridge_plan_v2_execute_results
         (attempt_id, step_id, result_ref, host_ref, input_logical_object_id,
          input_revision, result_schema_ref, result_digest, evidence_head, result_json, completed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![proposal.attempt_id, proposal.step_id, result.result_ref,
            result.host_ref.as_str(), result.input.logical_object_id, result.input.revision,
            result.result_schema_ref, result.result_digest, result.evidence_head,
            serde_json::to_string(result)?, now],
    )?;
    let changed = tx.execute(
        "UPDATE bridge_plan_v2_managed_step_claims SET state = 'completed', evidence_head = ?3, completed_at = ?4 WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'claimed'",
        params![proposal.attempt_id, proposal.step_id, proposal.evidence_head, now],
    )?;
    if changed != 1 {
        return invalid("Execute completion lost its exact one-use claim.");
    }
    tx.commit()?;
    Ok(())
}

fn fail_claim<T>(paths: &AppPaths, attempt_id: &str, step_id: &str, message: &str) -> AppResult<T> {
    mark_claim_failed(paths, attempt_id, step_id);
    invalid(message)
}

fn mark_claim_failed(paths: &AppPaths, attempt_id: &str, step_id: &str) {
    if let Ok(conn) = connection(paths) {
        let _ = conn.execute(
            "UPDATE bridge_plan_v2_managed_step_claims SET state = 'failed' WHERE attempt_id = ?1 AND step_id = ?2 AND state = 'claimed'",
            params![attempt_id, step_id],
        );
    }
}

pub(crate) fn interrupt_claim_for_run(paths: &AppPaths, run_ref: &ManagedRunRefV1) {
    if let Ok(conn) = connection(paths) {
        let _ = conn.execute(
            "UPDATE bridge_plan_v2_managed_step_claims SET state = 'interrupted' WHERE run_control_ref = ?1 AND state = 'claimed'",
            [run_ref.as_str()],
        );
    }
}

pub(crate) fn interrupt_claims_for_session(paths: &AppPaths, binding_ref: &str) {
    if let Ok(conn) = connection(paths) {
        let _ = conn.execute(
            "UPDATE bridge_plan_v2_managed_step_claims SET state = 'interrupted'
             WHERE state = 'claimed' AND attempt_id IN
             (SELECT attempt_id FROM bridge_plan_v2_attempts WHERE session_binding_ref = ?1)",
            [binding_ref],
        );
    }
}

fn validate_proposal_evidence(
    completion: &CompletionEvidenceV1,
    ids: &[String],
    head: &str,
) -> AppResult<()> {
    let expected = completion
        .evidence
        .iter()
        .map(|evidence| evidence.evidence_id.as_str().to_owned())
        .collect::<Vec<_>>();
    if ids != expected || head != completion.evidence_head {
        return invalid("Worker result evidence is incomplete, reordered, foreign, or forged.");
    }
    Ok(())
}

fn managed_step_contract(
    step: &PlanStepV2,
) -> AppResult<(
    ManagedSemanticOperationV1,
    crate::bridge_plan_v2::ManagedObjectRevisionV2,
    Option<u64>,
)> {
    match step {
        PlanStepV2::Transform { input, output, .. } => Ok((
            ManagedSemanticOperationV1::Transform,
            input.clone(),
            Some(output.revision),
        )),
        PlanStepV2::Execute { target, .. } => {
            Ok((ManagedSemanticOperationV1::Execute, target.clone(), None))
        }
        _ => invalid("Only Transform or Execute may create managed effect authority."),
    }
}

fn execute_schema_ref(step: &PlanStepV2) -> AppResult<String> {
    let PlanStepV2::Execute {
        execution_intent, ..
    } = step
    else {
        return invalid("Execute result schema requested for another primitive.");
    };
    domain_hash("pastey-execute-result-schema-v1", execution_intent)
}

fn current_authority(binding: &HostSessionBinding, now: i64) -> CurrentHostAuthorityV1 {
    CurrentHostAuthorityV1 {
        session_binding: binding.clone(),
        bridge_active: true,
        burned: false,
        disconnected: false,
        restarted: false,
        now,
    }
}

fn step_budgets() -> EffectBudgetsV1 {
    EffectBudgetsV1 {
        requests: 64,
        read_bytes: 16 * 1024 * 1024,
        write_bytes: 16 * 1024 * 1024,
        process_spawns: 8,
        process_signals: 8,
        cpu_millis: 60_000,
        memory_byte_millis: 8 * 1024 * 1024 * 60_000,
        wall_millis: 60_000,
        network_resolutions: 0,
        network_connections: 0,
        network_binds: 0,
        network_requests: 0,
        network_bytes: 0,
        network_time_millis: 0,
    }
}

fn resource_bounds() -> Vec<EffectBoundV1> {
    [
        ResourceVerbV1::Inspect,
        ResourceVerbV1::Read,
        ResourceVerbV1::Create,
        ResourceVerbV1::Replace,
    ]
    .into_iter()
    .map(|verb| EffectBoundV1 {
        capability: EffectCapabilityV1::Resource(verb),
        max_per_request: EffectBudgetsV1 {
            requests: 1,
            ..step_budgets()
        },
    })
    .collect()
}

fn process_bounds() -> Vec<EffectBoundV1> {
    [ProcessVerbV1::Spawn, ProcessVerbV1::Signal]
        .into_iter()
        .map(|verb| EffectBoundV1 {
            capability: EffectCapabilityV1::Process(verb),
            max_per_request: EffectBudgetsV1 {
                requests: 1,
                process_spawns: u64::from(verb == ProcessVerbV1::Spawn),
                process_signals: u64::from(verb == ProcessVerbV1::Signal),
                read_bytes: 32 * 1024,
                write_bytes: 1024 * 1024,
                cpu_millis: 30_000,
                memory_byte_millis: 8 * 1024 * 1024 * 30_000,
                wall_millis: 30_000,
                ..Default::default()
            },
        })
        .collect()
}

fn all_confinement_properties() -> BTreeSet<ConfinementPropertyV1> {
    [
        ConfinementPropertyV1::AuthorizedResourceProjection,
        ConfinementPropertyV1::AuthorityNeutralEnvironment,
        ConfinementPropertyV1::ExplicitProcessIo,
        ConfinementPropertyV1::PlatformSandboxedProcess,
        ConfinementPropertyV1::CancellableProcessSession,
        ConfinementPropertyV1::NoRawNetwork,
    ]
    .into_iter()
    .collect()
}

fn connection(paths: &AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn domain_hash<T: Serialize>(domain: &str, value: &T) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ATTACHMENT_VERSION.as_bytes());
    hasher.update(b"\0");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(&serde_json::to_vec(value)?);
    Ok(format!("{domain}:{}", hasher.finalize().to_hex()))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, path::PathBuf, sync::Arc};

    use base64::Engine as _;

    use super::*;
    use crate::{
        bridge_plan_v2::{
            seal_revision, AttemptStartDecisionV2, AttemptStartV2, BridgePlanV2Store,
            ManagedObjectRevisionV2, PlanApprovalV2, PlanRevisionV2, PlanRootV2, ReviewRequestV2,
            PLAN_SCHEMA_VERSION, PROTOCOL_VERSION,
        },
        config::StoredConfig,
        effect_authority::{
            lower_tool_request, BackendApplyV1, EffectPreconditionV1, EffectRequestKindV1,
            HostEffectBackendV1, ResourceEffectV1, StepWorkDescriptorV1, ToolEffectIntentV1,
            ToolRequestV1,
        },
        host_identity::{PlanParticipantRef, PlanParticipants},
        host_runtime::{HostEvent, HostEventSink, RuntimeTask, RuntimeTaskSpawner},
        managed_objects::{HostArtifactAcquisition, ManagedObjectAcquisitionKind},
        managed_resources::{ExecutableBindingSpecV1, HostManagedResourceBackendV1},
        models::LocalRole,
        storage,
        worker_harness::{
            WorkerProviderErrorKindV1, WorkerProviderErrorV1, WorkerProviderResponseV1,
            WorkerProviderTurnV1, WorkerProviderV1, WorkerResourceAliasV1, WorkerRunLimitsV1,
            WorkerToolCallV1,
        },
    };

    const NOW: i64 = 20_000;
    const BRIDGE: &str = "bridge-step8";

    #[derive(Default)]
    struct Sink;
    impl HostEventSink for Sink {
        fn emit(&self, _event: HostEvent) -> AppResult<()> {
            Ok(())
        }
    }
    #[derive(Default)]
    struct Spawner;
    impl RuntimeTaskSpawner for Spawner {
        fn spawn(&self, _task: RuntimeTask) {}
    }

    struct Fixture {
        runtime: HostRuntime,
        binding: HostSessionBinding,
        input: ManagedObjectAcquisition,
        revision: PlanRevisionV2,
        start: AttemptStartV2,
    }

    struct LostAfterIntent;
    impl HostEffectBackendV1 for LostAfterIntent {
        fn apply(&mut self, _request: &crate::effect_authority::EffectRequestV1) -> BackendApplyV1 {
            BackendApplyV1::LostAfterIntent
        }
    }

    struct ScriptedWorkerProvider {
        responses: VecDeque<Result<WorkerProviderResponseV1, WorkerProviderErrorV1>>,
        requests: Vec<crate::worker_harness::WorkerProviderRequestV1>,
    }

    impl WorkerProviderV1 for ScriptedWorkerProvider {
        fn next_turn(
            &mut self,
            request: crate::worker_harness::WorkerProviderRequestV1,
            _cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            self.requests.push(request);
            self.responses
                .pop_front()
                .expect("scripted Worker response")
                .map(WorkerProviderTurnV1::scripted)
        }
    }

    struct CancellingWorkerProvider;

    impl WorkerProviderV1 for CancellingWorkerProvider {
        fn next_turn(
            &mut self,
            _request: crate::worker_harness::WorkerProviderRequestV1,
            cancellation: &crate::worker_harness::WorkerHarnessRunV1,
        ) -> Result<WorkerProviderTurnV1, WorkerProviderErrorV1> {
            cancellation.cancel();
            Ok(WorkerProviderTurnV1::scripted(
                WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Read {
                        resource: WorkerResourceAliasV1::Input,
                    },
                },
            ))
        }
    }

    fn fixture(
        steps: impl FnOnce(&ManagedObjectAcquisition, &PlanParticipantRef) -> Vec<PlanStepV2>,
    ) -> Fixture {
        fixture_with_availability(steps, true)
    }

    fn fixture_with_availability(
        steps: impl FnOnce(&ManagedObjectAcquisition, &PlanParticipantRef) -> Vec<PlanStepV2>,
        managed_available: bool,
    ) -> Fixture {
        let root = std::env::temp_dir().join(format!("pastey-step8-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(root.clone(), root.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        storage::create_room(
            &paths,
            &crate::crypto::random_key(),
            "123456",
            5,
            LocalRole::Joined,
            Some(BRIDGE.into()),
            Some(NOW + 3_600),
        )
        .unwrap();
        let runtime = HostRuntime::new(
            paths.clone(),
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
                app_secret: crate::crypto::encode_key(&[9u8; 32]),
                device_id: "step8-local".into(),
            },
            Arc::new(Sink),
            Arc::new(Spawner),
        )
        .unwrap();
        let artifact_root = root.join("artifact");
        std::fs::create_dir_all(&artifact_root).unwrap();
        let artifact_path = artifact_root.join("input.txt");
        std::fs::write(&artifact_path, b"revision one").unwrap();
        let input = runtime
            .managed_objects
            .lock()
            .acquire_new(
                HostArtifactAcquisition {
                    kind: ManagedObjectAcquisitionKind::LocalSelection,
                    source_ref: "step8-test-input".into(),
                    bridge_id: Some(BRIDGE.into()),
                    path: artifact_path,
                    scope_root: artifact_root,
                    display_name: "input.txt".into(),
                    media_type: "text/plain".into(),
                    expires_at: NOW + 600,
                    app_owned_temporary: false,
                },
                NOW,
            )
            .unwrap();
        let requester_host = HostRef::from_device_id("step8-requester").unwrap();
        let plan_id = format!("plan-step8-{}", uuid::Uuid::new_v4());
        let participants = PlanParticipants::new(
            &plan_id,
            [requester_host.clone(), runtime.local_host_ref.clone()],
        )
        .unwrap();
        let requester = PlanParticipantRef::for_host(&plan_id, &requester_host).unwrap();
        let local = PlanParticipantRef::for_host(&plan_id, &runtime.local_host_ref).unwrap();
        let revision = seal_revision(PlanRevisionV2 {
            schema_version: PLAN_SCHEMA_VERSION.into(),
            plan_id: plan_id.clone(),
            revision_id: format!("revision-step8-{}", uuid::Uuid::new_v4()),
            revision_number: 1,
            revision_hash: String::new(),
            bridge_id: BRIDGE.into(),
            requester: requester.clone(),
            participants,
            roots: vec![PlanRootV2 {
                root_id: "root-input".into(),
                object: ManagedObjectRevisionV2 {
                    logical_object_id: input.object.logical_object_id.clone(),
                    revision: input.object.revision,
                },
                host: local.clone(),
            }],
            original_user_goal: "Apply exact approved managed work.".into(),
            expected_outcome: "Return a Core-validated result.".into(),
            steps: steps(&input, &local),
        })
        .unwrap();
        let approval = PlanApprovalV2 {
            approval_id: format!("approval-step8-{}", uuid::Uuid::new_v4()),
            plan_id: plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: BRIDGE.into(),
            requester: requester.clone(),
            expires_at: NOW + 500,
        };
        let review = ReviewRequestV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("review-step8-{}", uuid::Uuid::new_v4()),
            correlation_id: format!("correlation-step8-{}", uuid::Uuid::new_v4()),
            request_nonce: format!("nonce-step8-{}", uuid::Uuid::new_v4()),
            sender: requester.clone(),
            target: local.clone(),
            approval: approval.clone(),
            revision: revision.clone(),
        };
        let binding = HostSessionBinding::new(
            BRIDGE,
            runtime.local_host_ref.clone(),
            requester_host,
            "local-session",
            "requester-session",
            "peer-route",
            NOW + 500,
        )
        .unwrap();
        let store = BridgePlanV2Store::new(&paths);
        store.record_review(&review, &binding, NOW).unwrap();
        let start = AttemptStartV2 {
            protocol_version: PROTOCOL_VERSION.into(),
            message_id: format!("start-step8-{}", uuid::Uuid::new_v4()),
            correlation_id: review.correlation_id,
            request_nonce: review.request_nonce,
            attempt_id: format!("attempt-step8-{}", uuid::Uuid::new_v4()),
            approval_id: approval.approval_id,
            plan_id,
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: BRIDGE.into(),
            sender: requester,
            target: local,
            expires_at: NOW + 400,
        };
        let decision = if managed_available {
            store
                .accept_attempt_start_with_availability(
                    &start,
                    &binding,
                    &binding,
                    &runtime.host_admission,
                    ManagedPrimitiveAvailabilityV1::verified_attachment(
                        runtime.local_host_ref.clone(),
                        true,
                        true,
                    ),
                    NOW + 1,
                )
                .unwrap()
        } else {
            store
                .accept_attempt_start(&start, &binding, &binding, &runtime.host_admission, NOW + 1)
                .unwrap()
        };
        assert_eq!(
            matches!(decision, AttemptStartDecisionV2::Accepted(_)),
            managed_available
        );
        Fixture {
            runtime,
            binding,
            input,
            revision,
            start,
        }
    }

    fn transform_then_execute_steps(
        input: &ManagedObjectAcquisition,
        host: &PlanParticipantRef,
    ) -> Vec<PlanStepV2> {
        let one = ManagedObjectRevisionV2 {
            logical_object_id: input.object.logical_object_id.clone(),
            revision: 1,
        };
        let two = ManagedObjectRevisionV2 {
            logical_object_id: input.object.logical_object_id.clone(),
            revision: 2,
        };
        vec![
            PlanStepV2::Transform {
                step_id: "transform".into(),
                depends_on: vec![],
                host: host.clone(),
                input: one,
                output: two.clone(),
                modification_intent: "Rewrite safely.".into(),
            },
            PlanStepV2::Execute {
                step_id: "execute".into(),
                depends_on: vec!["transform".into()],
                host: host.clone(),
                target: two,
                execution_intent: "Validate output.".into(),
            },
        ]
    }

    fn claim(
        fixture: &Fixture,
        step_id: &str,
        input: ManagedObjectAcquisition,
    ) -> ManagedStepGrantV1 {
        fixture
            .runtime
            .claim_v2_managed_step(ManagedStepClaimRequestV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: step_id.into(),
                input,
                captured_binding: fixture.binding.clone(),
                current_binding: fixture.binding.clone(),
                now: NOW + 2,
                process_world: None,
            })
            .unwrap()
    }

    #[test]
    fn managed_workspace_projection_resolves_only_exact_envelope_resources() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let projection = grant.workspace.projection();
        let authority = fixture.runtime.effect_authority.lock();
        assert_eq!(
            grant
                .workspace
                .resolve(
                    &authority,
                    &projection,
                    WorkerWorkspaceAliasV1::Input,
                    crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                    ".",
                )
                .unwrap(),
            grant.input_handle
        );
        assert_eq!(
            grant
                .workspace
                .resolve(
                    &authority,
                    &projection,
                    WorkerWorkspaceAliasV1::Output,
                    crate::managed_workspace::WorkerWorkspaceOperationV1::Create,
                    "result.txt",
                )
                .unwrap(),
            grant.output_slot.clone().unwrap()
        );
        let encoded = serde_json::to_string(&projection).unwrap();
        for forbidden in [
            "handle",
            "hostRef",
            "session",
            "bridge",
            "envelope",
            "runControl",
            "safeIdentity",
            "transfer",
            "/Users/",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "leaked {forbidden}: {encoded}"
            );
        }
    }

    #[test]
    fn managed_workspace_rejects_selector_escape_alias_substitution_and_widening() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let projection = grant.workspace.projection();
        let authority = fixture.runtime.effect_authority.lock();
        for selector in [
            "/tmp/escape",
            "../escape",
            "nested/../escape",
            "file:///tmp/x",
        ] {
            assert!(grant
                .workspace
                .resolve(
                    &authority,
                    &projection,
                    WorkerWorkspaceAliasV1::Output,
                    crate::managed_workspace::WorkerWorkspaceOperationV1::Create,
                    selector,
                )
                .is_err());
        }
        assert!(grant
            .workspace
            .resolve(
                &authority,
                &projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Create,
                ".",
            )
            .is_err());
        assert!(grant
            .workspace
            .resolve(
                &authority,
                &projection,
                WorkerWorkspaceAliasV1::Workspace,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());
        assert!(grant
            .workspace
            .resolve(
                &authority,
                &projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                "child.txt",
            )
            .is_err());
        let mut widened = projection.clone();
        widened.resources[0]
            .operations
            .insert(crate::managed_workspace::WorkerWorkspaceOperationV1::Create);
        assert!(grant
            .workspace
            .resolve(
                &authority,
                &widened,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());
    }

    #[test]
    fn managed_workspace_rejects_cross_run_handle_projection_and_host_substitution() {
        let first = fixture(transform_then_execute_steps);
        let first_grant = claim(&first, "transform", first.input.clone());
        let stale_projection = first_grant.workspace.projection();
        let second = fixture(transform_then_execute_steps);
        let second_grant = claim(&second, "transform", second.input.clone());
        let second_authority = second.runtime.effect_authority.lock();
        assert!(second_grant
            .workspace
            .resolve(
                &second_authority,
                &stale_projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());
        assert!(ManagedRunWorkspaceV1::derive(
            &second_authority,
            second_grant.access.clone(),
            vec![ManagedWorkspaceResourceAttachmentV1 {
                alias: WorkerWorkspaceAliasV1::Input,
                kind: ResourceKindV1::ManagedRevision,
                handle_ref: first_grant.input_handle.clone(),
            }],
            None,
        )
        .is_err());
        assert!(ManagedRunWorkspaceV1::derive(
            &second_authority,
            second_grant.access.clone(),
            vec![
                ManagedWorkspaceResourceAttachmentV1 {
                    alias: WorkerWorkspaceAliasV1::Input,
                    kind: ResourceKindV1::ManagedRevision,
                    handle_ref: second_grant.input_handle.clone(),
                },
                ManagedWorkspaceResourceAttachmentV1 {
                    alias: WorkerWorkspaceAliasV1::Output,
                    kind: ResourceKindV1::OutputSlot,
                    handle_ref: first_grant.output_slot.clone().unwrap(),
                },
            ],
            None,
        )
        .is_err());
        let mut wrong_host_access = second_grant.access.clone();
        wrong_host_access.context.host_ref =
            HostRef::from_device_id("workspace-other-host").unwrap();
        assert!(ManagedRunWorkspaceV1::derive(
            &second_authority,
            wrong_host_access,
            vec![ManagedWorkspaceResourceAttachmentV1 {
                alias: WorkerWorkspaceAliasV1::Input,
                kind: ResourceKindV1::ManagedRevision,
                handle_ref: second_grant.input_handle.clone(),
            }],
            None,
        )
        .is_err());
    }

    #[test]
    fn cancellation_burn_and_restart_make_old_workspace_projections_unusable() {
        let cancelled = fixture(transform_then_execute_steps);
        let cancelled_grant = claim(&cancelled, "transform", cancelled.input.clone());
        let cancelled_projection = cancelled_grant.workspace.projection();
        cancelled
            .runtime
            .cancel_managed_run(&cancelled_grant.access.run_control_ref)
            .unwrap();
        assert!(cancelled_grant
            .workspace
            .resolve(
                &cancelled.runtime.effect_authority.lock(),
                &cancelled_projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());

        let burned = fixture(transform_then_execute_steps);
        let burned_grant = claim(&burned, "transform", burned.input.clone());
        let burned_projection = burned_grant.workspace.projection();
        burned.runtime.purge_room(BRIDGE);
        assert!(burned_grant
            .workspace
            .resolve(
                &burned.runtime.effect_authority.lock(),
                &burned_projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());

        let disconnected = fixture(transform_then_execute_steps);
        let disconnected_grant = claim(&disconnected, "transform", disconnected.input.clone());
        let disconnected_projection = disconnected_grant.workspace.projection();
        disconnected
            .runtime
            .revoke_managed_session(&disconnected.binding.binding_ref);
        assert!(disconnected_grant
            .workspace
            .resolve(
                &disconnected.runtime.effect_authority.lock(),
                &disconnected_projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());

        let restarted = fixture(transform_then_execute_steps);
        let restarted_grant = claim(&restarted, "transform", restarted.input.clone());
        let restarted_projection = restarted_grant.workspace.projection();
        restarted.runtime.shutdown_all();
        assert!(restarted_grant
            .workspace
            .resolve(
                &restarted.runtime.effect_authority.lock(),
                &restarted_projection,
                WorkerWorkspaceAliasV1::Input,
                crate::managed_workspace::WorkerWorkspaceOperationV1::Read,
                ".",
            )
            .is_err());
    }

    fn requests_for_output(
        grant: &ManagedStepGrantV1,
        bytes: &[u8],
    ) -> Vec<crate::effect_authority::EffectRequestV1> {
        let output = grant.output_slot.clone().unwrap();
        let descriptor = StepWorkDescriptorV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            context: grant.access.context.clone(),
            envelope_ref: grant.access.envelope_ref.clone(),
            run_control_ref: grant.access.run_control_ref.clone(),
            first_sequence: 0,
        };
        lower_tool_request(
            &descriptor,
            &ToolRequestV1 {
                tool_name: "synthetic-different-tools-have-no-authority".into(),
                adapter_version_ref: "step8-test-adapter-v1".into(),
                intents: vec![
                    ToolEffectIntentV1 {
                        effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                            verb: ResourceVerbV1::Read,
                            handle_ref: grant.input_handle.clone(),
                            relative_selector: ".".into(),
                            value_digest: None,
                        }),
                        requested_budget_slice: EffectBudgetsV1 {
                            requests: 1,
                            read_bytes: 12,
                            ..Default::default()
                        },
                        preconditions: vec![],
                    },
                    ToolEffectIntentV1 {
                        effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                            verb: ResourceVerbV1::Create,
                            handle_ref: output,
                            relative_selector: "result.txt".into(),
                            value_digest: Some(blake3::hash(bytes).to_hex().to_string()),
                        }),
                        requested_budget_slice: EffectBudgetsV1 {
                            requests: 1,
                            write_bytes: bytes.len() as u64,
                            ..Default::default()
                        },
                        preconditions: vec![],
                    },
                ],
            },
        )
        .unwrap()
    }

    fn produce_transform(
        fixture: &Fixture,
        grant: &ManagedStepGrantV1,
    ) -> (
        TransformResultProposalV1,
        Vec<crate::effect_authority::EffectEvidenceV1>,
    ) {
        let output_bytes = b"revision two";
        let requests = requests_for_output(grant, output_bytes);
        let digest = blake3::hash(output_bytes).to_hex().to_string();
        let mut authority = fixture.runtime.effect_authority.lock();
        let mut resolver = fixture.runtime.managed_resources.lock();
        let mut objects = fixture.runtime.managed_objects.lock();
        resolver
            .stage_write_payload(
                &authority,
                &grant.access,
                grant.output_slot.as_ref().unwrap(),
                &digest,
                output_bytes.to_vec(),
            )
            .unwrap();
        let mut evidence = Vec::new();
        for request in &requests {
            let mut backend =
                HostManagedResourceBackendV1::new(&mut resolver, &mut objects, NOW + 3);
            evidence.push(
                authority
                    .enforce(request, &grant.access.current, &mut backend)
                    .unwrap(),
            );
        }
        let seal = resolver
            .seal_output_slot(
                &authority,
                &grant.access,
                grant.output_slot.as_ref().unwrap(),
                "result.txt",
                evidence.last().unwrap(),
            )
            .unwrap();
        drop(objects);
        drop(resolver);
        drop(authority);
        let ids = evidence
            .iter()
            .map(|item| item.evidence_id.as_str().to_owned())
            .collect();
        let head = evidence.last().unwrap().evidence_digest.clone();
        (
            TransformResultProposalV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: "transform".into(),
                context_ref: grant.access.context.context_ref().unwrap(),
                envelope_ref: grant.access.envelope_ref.clone(),
                run_control_ref: grant.access.run_control_ref.clone(),
                input: grant.access.context.input_revisions[0].clone(),
                output: ManagedObjectRevisionResultV1 {
                    logical_object_id: fixture.input.object.logical_object_id.clone(),
                    revision: 2,
                    host_ref: fixture.runtime.local_host_ref.clone(),
                    content_digest: digest,
                },
                output_seal: seal,
                evidence_ids: ids,
                evidence_head: head,
                display_name: "result.txt".into(),
                media_type: "text/plain".into(),
            },
            evidence,
        )
    }

    fn worker_claim_request(fixture: &Fixture) -> ManagedStepClaimRequestV1 {
        ManagedStepClaimRequestV1 {
            attempt_id: fixture.start.attempt_id.clone(),
            step_id: "transform".into(),
            input: fixture.input.clone(),
            captured_binding: fixture.binding.clone(),
            current_binding: fixture.binding.clone(),
            now: NOW + 2,
            process_world: None,
        }
    }

    fn worker_process_claim_request(
        fixture: &Fixture,
        step_id: &str,
        input: ManagedObjectAcquisition,
        executable: &str,
    ) -> ManagedStepClaimRequestV1 {
        ManagedStepClaimRequestV1 {
            attempt_id: fixture.start.attempt_id.clone(),
            step_id: step_id.into(),
            input,
            captured_binding: fixture.binding.clone(),
            current_binding: fixture.binding.clone(),
            now: NOW + 2,
            process_world: Some(ManagedProcessWorldSpecV1 {
                executable: ExecutableBindingSpecV1 {
                    executable_path: PathBuf::from(executable),
                    scope_root: PathBuf::from("/usr/bin"),
                },
            }),
        }
    }

    fn worker_script(output: &[u8]) -> ScriptedWorkerProvider {
        ScriptedWorkerProvider {
            responses: VecDeque::from([
                Err(WorkerProviderErrorV1 {
                    kind: WorkerProviderErrorKindV1::Retryable,
                }),
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Read {
                        resource: WorkerResourceAliasV1::Input,
                    },
                }),
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Create {
                        relative_selector: "result.txt".into(),
                        content_base64: base64::engine::general_purpose::STANDARD.encode(output),
                    },
                }),
                Ok(WorkerProviderResponseV1::Final {
                    output_selector: "result.txt".into(),
                    display_name: "result.txt".into(),
                    media_type: "text/plain".into(),
                }),
            ]),
            requests: Vec::new(),
        }
    }

    #[test]
    fn worker_harness_runs_one_same_host_transform_through_resource_effects_only() {
        let fixture = fixture(transform_then_execute_steps);
        let mut provider = worker_script(b"revision two from worker");
        let result = fixture
            .runtime
            .run_v2_transform_worker(
                worker_claim_request(&fixture),
                WorkerRunLimitsV1::default(),
                &mut provider,
            )
            .unwrap();
        assert_eq!(result.object.revision, 2);
        assert_eq!(result.object.host_ref, fixture.runtime.local_host_ref);
        let artifact = fixture
            .runtime
            .managed_objects
            .lock()
            .resolve(&result, NOW + 3)
            .unwrap();
        assert_eq!(
            std::fs::read(artifact.path).unwrap(),
            b"revision two from worker"
        );
        assert_eq!(provider.requests.len(), 4);
        assert!(provider.requests[0]
            .system_instructions
            .contains("cannot claim work"));
        assert!(provider.requests[1].history.iter().any(|turn| matches!(
            turn.observation,
            Some(crate::worker_harness::WorkerObservationV1::ProviderRetry { .. })
        )));
        let tool_schemas = serde_json::to_string(&provider.requests[0].tools).unwrap();
        assert!(tool_schemas.contains("resource_read"));
        assert!(!tool_schemas.contains("process"));
        assert!(!tool_schemas.contains("network"));
        assert!(!tool_schemas.contains("artifact"));
        let workspace = serde_json::to_string(&provider.requests[0].workspace).unwrap();
        assert!(workspace.contains("pastey-managed-workspace-v1"));
        assert!(workspace.contains("\"alias\":\"input\""));
        assert!(workspace.contains("\"alias\":\"output\""));
        assert!(!workspace.contains("handle"));
        assert!(!workspace.contains("hostRef"));
        assert!(!workspace.contains("runControl"));
        assert!(!workspace.contains("transfer"));
    }

    #[test]
    fn worker_cancellation_prevents_dispatch_and_interrupts_the_claim() {
        let fixture = fixture(transform_then_execute_steps);
        let mut provider = CancellingWorkerProvider;
        assert!(fixture
            .runtime
            .run_v2_transform_worker(
                worker_claim_request(&fixture),
                WorkerRunLimitsV1::default(),
                &mut provider,
            )
            .is_err());
        let state: String = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT state FROM bridge_plan_v2_managed_step_claims WHERE attempt_id = ?1 AND step_id = 'transform'",
                [&fixture.start.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "interrupted");
        let results: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_transform_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(results, 0);
    }

    #[test]
    fn provider_context_overflow_compacts_once_then_retries_without_authority_change() {
        let fixture = fixture(transform_then_execute_steps);
        let output = b"after compacted provider context";
        let mut provider = ScriptedWorkerProvider {
            responses: VecDeque::from([
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Read {
                        resource: WorkerResourceAliasV1::Input,
                    },
                }),
                Err(WorkerProviderErrorV1 {
                    kind: WorkerProviderErrorKindV1::ContextOverflow,
                }),
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Create {
                        relative_selector: "result.txt".into(),
                        content_base64: base64::engine::general_purpose::STANDARD.encode(output),
                    },
                }),
                Ok(WorkerProviderResponseV1::Final {
                    output_selector: "result.txt".into(),
                    display_name: "result.txt".into(),
                    media_type: "text/plain".into(),
                }),
            ]),
            requests: Vec::new(),
        };
        let result = fixture.runtime.run_v2_transform_worker(
            worker_claim_request(&fixture),
            WorkerRunLimitsV1::default(),
            &mut provider,
        );
        assert!(result.is_ok());
        assert!(provider.requests[2].history.iter().any(|turn| matches!(
            turn.observation,
            Some(crate::worker_harness::WorkerObservationV1::Compacted { .. })
        )));
        let projected = serde_json::to_string(&provider.requests[2]).unwrap();
        assert!(!projected.contains("NetworkGrant"));
        assert!(!projected.contains("ResourceHandleRef"));
    }

    #[test]
    fn process_failure_becomes_observation_then_worker_self_corrects_with_resource_output() {
        let fixture = fixture(transform_then_execute_steps);
        if !fixture
            .runtime
            .execution_worlds
            .platform_availability()
            .available
        {
            return;
        }
        let output = b"corrected after contained failure";
        let mut provider = ScriptedWorkerProvider {
            responses: VecDeque::from([
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::ProcessSpawn {
                        arguments: vec![],
                        environment: Default::default(),
                        stdin_base64: None,
                        working_directory: None,
                    },
                }),
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::Create {
                        relative_selector: "result.txt".into(),
                        content_base64: base64::engine::general_purpose::STANDARD.encode(output),
                    },
                }),
                Ok(WorkerProviderResponseV1::Final {
                    output_selector: "result.txt".into(),
                    display_name: "result.txt".into(),
                    media_type: "text/plain".into(),
                }),
            ]),
            requests: Vec::new(),
        };
        let result = fixture.runtime.run_v2_transform_worker(
            worker_process_claim_request(
                &fixture,
                "transform",
                fixture.input.clone(),
                "/usr/bin/false",
            ),
            WorkerRunLimitsV1::default(),
            &mut provider,
        );
        assert!(result.is_ok());
        assert!(provider.requests[1].history.iter().any(|turn| matches!(
            turn.observation,
            Some(crate::worker_harness::WorkerObservationV1::Process {
                state: Some(ref state),
                exit_code: Some(1),
                ..
            }) if state == "failed"
        )));
        let workspace = serde_json::to_string(&provider.requests[0].workspace).unwrap();
        assert!(workspace.contains("\"alias\":\"scratch\""));
        assert!(!workspace.contains("handle"));
        assert!(!workspace.contains("path"));
    }

    #[test]
    fn process_worker_fails_closed_when_the_verified_world_is_unavailable() {
        let fixture = fixture(transform_then_execute_steps);
        if fixture
            .runtime
            .execution_worlds
            .platform_availability()
            .available
        {
            return;
        }
        let mut provider = ScriptedWorkerProvider {
            responses: VecDeque::new(),
            requests: Vec::new(),
        };
        assert!(fixture
            .runtime
            .run_v2_transform_worker(
                worker_process_claim_request(
                    &fixture,
                    "transform",
                    fixture.input.clone(),
                    "/usr/bin/false",
                ),
                WorkerRunLimitsV1::default(),
                &mut provider,
            )
            .is_err());
        assert!(provider.requests.is_empty());
    }

    #[test]
    fn execute_worker_records_result_without_managed_lineage() {
        let fixture = fixture(transform_then_execute_steps);
        if !fixture
            .runtime
            .execution_worlds
            .platform_availability()
            .available
        {
            return;
        }
        let transform_grant = claim(&fixture, "transform", fixture.input.clone());
        let (proposal, _) = produce_transform(&fixture, &transform_grant);
        let transformed = fixture
            .runtime
            .finalize_v2_transform(proposal, fixture.binding.clone(), NOW + 4)
            .unwrap();
        let mut provider = ScriptedWorkerProvider {
            responses: VecDeque::from([
                Ok(WorkerProviderResponseV1::ToolCall {
                    call: WorkerToolCallV1::ProcessSpawn {
                        arguments: vec![],
                        environment: Default::default(),
                        stdin_base64: None,
                        working_directory: None,
                    },
                }),
                Ok(WorkerProviderResponseV1::FinalExecute),
            ]),
            requests: Vec::new(),
        };
        let result = fixture
            .runtime
            .run_v2_execute_worker(
                worker_process_claim_request(
                    &fixture,
                    "execute",
                    transformed.clone(),
                    "/usr/bin/true",
                ),
                WorkerRunLimitsV1::default(),
                &mut provider,
            )
            .unwrap();
        assert_eq!(
            result.input.logical_object_id,
            transformed.object.logical_object_id
        );
        assert_eq!(result.input.revision, transformed.object.revision);
        let revisions: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_transform_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revisions, 1);
    }

    #[test]
    fn exact_claim_is_one_use_and_rejects_session_host_step_and_revision_substitution() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        assert_eq!(grant.operation, ManagedSemanticOperationV1::Transform);
        assert!(fixture
            .runtime
            .claim_v2_managed_step(ManagedStepClaimRequestV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: "transform".into(),
                input: fixture.input.clone(),
                captured_binding: fixture.binding.clone(),
                current_binding: fixture.binding.clone(),
                now: NOW + 3,
                process_world: None,
            })
            .is_err());
        let mut wrong_revision = fixture.input.clone();
        wrong_revision.object.revision = 2;
        assert!(fixture
            .runtime
            .claim_v2_managed_step(ManagedStepClaimRequestV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: "execute".into(),
                input: wrong_revision,
                captured_binding: fixture.binding.clone(),
                current_binding: fixture.binding.clone(),
                now: NOW + 3,
                process_world: None,
            })
            .is_err());
        let wrong_session = HostSessionBinding::new(
            BRIDGE,
            fixture.runtime.local_host_ref.clone(),
            fixture.binding.peer_host_ref.clone(),
            "other-local",
            "requester-session",
            "other-route",
            NOW + 500,
        )
        .unwrap();
        assert!(fixture
            .runtime
            .claim_v2_managed_step(ManagedStepClaimRequestV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: "execute".into(),
                input: fixture.input.clone(),
                captured_binding: fixture.binding.clone(),
                current_binding: wrong_session,
                now: NOW + 3,
                process_world: None,
            })
            .is_err());
    }

    #[test]
    fn transform_registers_exact_n_plus_one_and_only_then_continuation_becomes_eligible() {
        let fixture = fixture(transform_then_execute_steps);
        assert!(fixture
            .runtime
            .claim_v2_managed_step(ManagedStepClaimRequestV1 {
                attempt_id: fixture.start.attempt_id.clone(),
                step_id: "execute".into(),
                input: fixture.input.clone(),
                captured_binding: fixture.binding.clone(),
                current_binding: fixture.binding.clone(),
                now: NOW + 2,
                process_world: None,
            })
            .is_err());
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let (proposal, _) = produce_transform(&fixture, &grant);
        let output = fixture
            .runtime
            .finalize_v2_transform(proposal, fixture.binding.clone(), NOW + 4)
            .unwrap();
        assert_eq!(output.object.revision, 2);
        assert_eq!(
            output.object.logical_object_id,
            fixture.input.object.logical_object_id
        );
        assert_eq!(output.object.host_ref, fixture.runtime.local_host_ref);
        let execute = claim(&fixture, "execute", output);
        assert_eq!(execute.operation, ManagedSemanticOperationV1::Execute);
        assert!(execute.output_slot.is_none());
    }

    #[test]
    fn forged_reordered_foreign_evidence_and_output_identity_cannot_finalize() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let (proposal, evidence) = produce_transform(&fixture, &grant);
        assert_eq!(evidence.len(), 2);
        let mut reordered = proposal.clone();
        reordered.evidence_ids.reverse();
        assert!(fixture
            .runtime
            .finalize_v2_transform(reordered, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let mut forged = proposal.clone();
        forged.evidence_head = "worker-forged-success".into();
        assert!(fixture
            .runtime
            .finalize_v2_transform(forged, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let mut incomplete = proposal.clone();
        incomplete.evidence_ids.pop();
        assert!(fixture
            .runtime
            .finalize_v2_transform(incomplete, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let mut wrong_lineage = proposal.clone();
        wrong_lineage.output.revision = 1;
        assert!(fixture
            .runtime
            .finalize_v2_transform(wrong_lineage, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let mut wrong_slot = proposal;
        wrong_slot.output_seal.handle_ref = grant.input_handle;
        assert!(fixture
            .runtime
            .finalize_v2_transform(wrong_slot, fixture.binding.clone(), NOW + 4,)
            .is_err());
    }

    #[test]
    fn output_slot_and_safe_identity_substitution_fail_before_lineage_registration() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let (mut proposal, _) = produce_transform(&fixture, &grant);
        proposal.output_seal.content_digest = "substituted-safe-identity".into();
        proposal.output.content_digest = proposal.output_seal.content_digest.clone();
        assert!(fixture
            .runtime
            .finalize_v2_transform(proposal, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let results: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM bridge_plan_v2_transform_results",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(results, 0);
    }

    #[test]
    fn cancellation_restart_and_whole_plan_unavailability_fail_closed() {
        let primary = fixture(transform_then_execute_steps);
        let grant = claim(&primary, "transform", primary.input.clone());
        let (proposal, _) = produce_transform(&primary, &grant);
        primary
            .runtime
            .cancel_managed_run(&grant.access.run_control_ref)
            .unwrap();
        assert!(primary
            .runtime
            .finalize_v2_transform(proposal, primary.binding.clone(), NOW + 4,)
            .is_err());
        let state: String = connection(&primary.runtime.paths).unwrap().query_row(
            "SELECT state FROM bridge_plan_v2_managed_step_claims WHERE attempt_id = ?1 AND step_id = 'transform'",
            [&primary.start.attempt_id], |row| row.get(0),
        ).unwrap();
        assert_eq!(state, "interrupted");

        let unavailable = fixture_with_availability(
            |input, host| {
                vec![PlanStepV2::Transform {
                    step_id: "transform".into(),
                    depends_on: vec![],
                    host: host.clone(),
                    input: ManagedObjectRevisionV2 {
                        logical_object_id: input.object.logical_object_id.clone(),
                        revision: 1,
                    },
                    output: ManagedObjectRevisionV2 {
                        logical_object_id: input.object.logical_object_id.clone(),
                        revision: 2,
                    },
                    modification_intent: "No backend fallback.".into(),
                }]
            },
            false,
        );
        let attempts: i64 = connection(&unavailable.runtime.paths)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM bridge_plan_v2_attempts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn execute_records_authoritative_result_without_object_lineage() {
        let fixture = fixture(|input, host| {
            vec![PlanStepV2::Execute {
                step_id: "execute".into(),
                depends_on: vec![],
                host: host.clone(),
                target: ManagedObjectRevisionV2 {
                    logical_object_id: input.object.logical_object_id.clone(),
                    revision: 1,
                },
                execution_intent: "Inspect exact revision.".into(),
            }]
        });
        let grant = claim(&fixture, "execute", fixture.input.clone());
        let descriptor = StepWorkDescriptorV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            context: grant.access.context.clone(),
            envelope_ref: grant.access.envelope_ref.clone(),
            run_control_ref: grant.access.run_control_ref.clone(),
            first_sequence: 0,
        };
        let request = lower_tool_request(
            &descriptor,
            &ToolRequestV1 {
                tool_name: "synthetic-inspector".into(),
                adapter_version_ref: "step8-test-adapter-v1".into(),
                intents: vec![ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                        verb: ResourceVerbV1::Read,
                        handle_ref: grant.input_handle.clone(),
                        relative_selector: ".".into(),
                        value_digest: None,
                    }),
                    requested_budget_slice: EffectBudgetsV1 {
                        requests: 1,
                        read_bytes: 12,
                        ..Default::default()
                    },
                    preconditions: Vec::<EffectPreconditionV1>::new(),
                }],
            },
        )
        .unwrap()
        .remove(0);
        let evidence = {
            let mut authority = fixture.runtime.effect_authority.lock();
            let mut resolver = fixture.runtime.managed_resources.lock();
            let mut objects = fixture.runtime.managed_objects.lock();
            let mut backend =
                HostManagedResourceBackendV1::new(&mut resolver, &mut objects, NOW + 3);
            authority
                .enforce(&request, &grant.access.current, &mut backend)
                .unwrap()
        };
        let schema = execute_schema_ref(&fixture.revision.steps[0]).unwrap();
        let proposal = ExecuteResultProposalV1 {
            attempt_id: fixture.start.attempt_id.clone(),
            step_id: "execute".into(),
            context_ref: grant.access.context.context_ref().unwrap(),
            envelope_ref: grant.access.envelope_ref.clone(),
            run_control_ref: grant.access.run_control_ref.clone(),
            input: grant.access.context.input_revisions[0].clone(),
            evidence_ids: vec![evidence.evidence_id.as_str().to_owned()],
            evidence_head: evidence.evidence_digest,
            result_schema_ref: schema,
            result_digest: "execute-result-digest".into(),
            status: "completed".into(),
        };
        let result = fixture
            .runtime
            .finalize_v2_execute(proposal.clone(), fixture.binding.clone(), NOW + 4)
            .unwrap();
        assert_eq!(result.input.revision, 1);
        assert!(fixture
            .runtime
            .finalize_v2_execute(proposal, fixture.binding.clone(), NOW + 4,)
            .is_err());
        let (execute_results, transform_results): (i64, i64) = connection(&fixture.runtime.paths)
            .unwrap().query_row(
                "SELECT (SELECT COUNT(*) FROM bridge_plan_v2_execute_results), (SELECT COUNT(*) FROM bridge_plan_v2_transform_results)",
                [], |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
        assert_eq!((execute_results, transform_results), (1, 0));
    }

    #[test]
    fn indeterminate_write_ahead_restart_and_burn_never_finalize_authority() {
        let fixture = fixture(transform_then_execute_steps);
        let grant = claim(&fixture, "transform", fixture.input.clone());
        let request = requests_for_output(&grant, b"never authoritative").remove(0);
        assert!(fixture
            .runtime
            .effect_authority
            .lock()
            .enforce(&request, &grant.access.current, &mut LostAfterIntent,)
            .is_err());
        assert!(fixture
            .runtime
            .effect_authority
            .lock()
            .completion_evidence(
                &grant.access.run_control_ref,
                &grant.access.envelope_ref,
                &grant.access.current,
            )
            .is_err());

        crate::bridge_plan_v2::reconcile_startup(&fixture.runtime.paths).unwrap();
        let (attempt_state, claim_state): (String, String) = connection(&fixture.runtime.paths)
            .unwrap().query_row(
                "SELECT attempts.state, claims.state FROM bridge_plan_v2_attempts AS attempts JOIN bridge_plan_v2_managed_step_claims AS claims ON claims.attempt_id = attempts.attempt_id WHERE attempts.attempt_id = ?1",
                [&fixture.start.attempt_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
        assert_eq!(
            (attempt_state.as_str(), claim_state.as_str()),
            ("interrupted", "interrupted")
        );

        let inbox = fixture.runtime.paths.app_data_dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        storage::burn_room(&fixture.runtime.paths, BRIDGE, &inbox).unwrap();
        fixture.runtime.purge_room(BRIDGE);
        let remaining: i64 = connection(&fixture.runtime.paths)
            .unwrap()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM bridge_plan_v2_managed_step_claims) +
                    (SELECT COUNT(*) FROM bridge_plan_v2_transform_results) +
                    (SELECT COUNT(*) FROM bridge_plan_v2_execute_results)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}

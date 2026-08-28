//! Host-local managed workspace ABI for one exact Worker run.
//!
//! `ManagedRunWorkspaceV1` aggregates resource attachments already present in
//! an active `EffectEnvelopeV1`. It mints no handle, grant, verb, budget, or
//! execution authority. Its serializable projection contains only bounded
//! aliases and facts; every resolution revalidates the underlying attachment
//! before the existing effect path may act.

#![allow(dead_code)] // Workspace/scratch aliases attach incrementally.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    effect_authority::{
        EffectAuthorityStateV1, ResourceGrantV1, ResourceHandleRefV1, ResourceKindV1,
        ResourceVerbV1,
    },
    error::{AppError, AppResult},
    managed_resources::{validate_managed_resource_selector, ManagedResourceAccessV1},
};

const MANAGED_WORKSPACE_VERSION: &str = "pastey-managed-workspace-v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerWorkspaceAliasV1 {
    Input,
    Workspace,
    Output,
    Scratch,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerWorkspaceResourceKindV1 {
    ManagedRevision,
    Workspace,
    Output,
    Scratch,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerWorkspaceOperationV1 {
    Inspect,
    Read,
    Create,
    Replace,
}

impl WorkerWorkspaceOperationV1 {
    fn resource_verb(self) -> ResourceVerbV1 {
        match self {
            Self::Inspect => ResourceVerbV1::Inspect,
            Self::Read => ResourceVerbV1::Read,
            Self::Create => ResourceVerbV1::Create,
            Self::Replace => ResourceVerbV1::Replace,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerWorkspaceResourceProjectionV1 {
    pub(crate) alias: WorkerWorkspaceAliasV1,
    pub(crate) kind: WorkerWorkspaceResourceKindV1,
    pub(crate) operations: BTreeSet<WorkerWorkspaceOperationV1>,
    pub(crate) relative_selectors: bool,
}

/// Non-authoritative model view. `projection_ref` is deliberately omitted from
/// serialization: it binds the in-process value to its lease but gives the
/// provider no token or handle to replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerWorkspaceProjectionV1 {
    pub(crate) schema_version: String,
    pub(crate) resources: Vec<WorkerWorkspaceResourceProjectionV1>,
    #[serde(skip_serializing)]
    projection_ref: String,
}

impl WorkerWorkspaceProjectionV1 {
    pub(crate) fn resources_for(
        &self,
        operation: WorkerWorkspaceOperationV1,
    ) -> Vec<WorkerWorkspaceAliasV1> {
        self.resources
            .iter()
            .filter(|resource| resource.operations.contains(&operation))
            .map(|resource| resource.alias)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            schema_version: MANAGED_WORKSPACE_VERSION.into(),
            resources: Vec::new(),
            projection_ref: "test-empty-workspace-projection".into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn input_output_for_test(has_output: bool) -> Self {
        let mut resources = vec![WorkerWorkspaceResourceProjectionV1 {
            alias: WorkerWorkspaceAliasV1::Input,
            kind: WorkerWorkspaceResourceKindV1::ManagedRevision,
            operations: [
                WorkerWorkspaceOperationV1::Inspect,
                WorkerWorkspaceOperationV1::Read,
            ]
            .into_iter()
            .collect(),
            relative_selectors: false,
        }];
        if has_output {
            resources.push(WorkerWorkspaceResourceProjectionV1 {
                alias: WorkerWorkspaceAliasV1::Output,
                kind: WorkerWorkspaceResourceKindV1::Output,
                operations: [
                    WorkerWorkspaceOperationV1::Inspect,
                    WorkerWorkspaceOperationV1::Read,
                    WorkerWorkspaceOperationV1::Create,
                    WorkerWorkspaceOperationV1::Replace,
                ]
                .into_iter()
                .collect(),
                relative_selectors: true,
            });
        }
        Self {
            schema_version: MANAGED_WORKSPACE_VERSION.into(),
            resources,
            projection_ref: format!("test-input-output-workspace-projection:{has_output}"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedWorkspaceResourceAttachmentV1 {
    pub(crate) alias: WorkerWorkspaceAliasV1,
    pub(crate) kind: ResourceKindV1,
    pub(crate) handle_ref: ResourceHandleRefV1,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedWorkspaceProcessBindingV1 {
    pub(crate) world_ref: crate::effect_authority::ExecutionWorldRefV1,
    pub(crate) executable_handle: ResourceHandleRefV1,
}

#[derive(Clone, Debug)]
struct ManagedWorkspaceResourceLeaseV1 {
    grant: ResourceGrantV1,
    effective_verbs: BTreeSet<ResourceVerbV1>,
    projection: WorkerWorkspaceResourceProjectionV1,
}

/// Host-private lifetime aggregation over an already active run. This is not a
/// grant and cannot widen the envelope from which its attachments were derived.
#[derive(Clone, Debug)]
pub(crate) struct ManagedRunWorkspaceV1 {
    access: ManagedResourceAccessV1,
    resources: BTreeMap<WorkerWorkspaceAliasV1, ManagedWorkspaceResourceLeaseV1>,
    process: Option<ManagedWorkspaceProcessBindingV1>,
    projection: WorkerWorkspaceProjectionV1,
}

impl ManagedRunWorkspaceV1 {
    pub(crate) fn derive(
        authority: &EffectAuthorityStateV1,
        access: ManagedResourceAccessV1,
        attachments: Vec<ManagedWorkspaceResourceAttachmentV1>,
        process: Option<ManagedWorkspaceProcessBindingV1>,
    ) -> AppResult<Self> {
        if attachments.is_empty() {
            return invalid("Managed workspace requires at least one exact resource attachment.");
        }
        let mut resources = BTreeMap::new();
        for attachment in attachments {
            validate_alias_kind(attachment.alias, attachment.kind)?;
            if resources.contains_key(&attachment.alias) {
                return invalid("Managed workspace resource alias is duplicated.");
            }
            let (grant, effective_verbs) = authority.validate_resource_projection_attachment(
                &attachment.handle_ref,
                attachment.kind,
                &access.envelope_ref,
                &access.run_control_ref,
                &access.context,
                &access.current,
            )?;
            let projection = project_resource(attachment.alias, &grant, &effective_verbs);
            resources.insert(
                attachment.alias,
                ManagedWorkspaceResourceLeaseV1 {
                    grant,
                    effective_verbs,
                    projection,
                },
            );
        }
        if !resources.contains_key(&WorkerWorkspaceAliasV1::Input) {
            return invalid("Managed workspace requires the exact input revision alias.");
        }
        if let Some(process) = &process {
            let (world, _) = authority.validate_execution_world_attachment(
                &process.world_ref,
                &access.envelope_ref,
                &access.run_control_ref,
                &access.context,
                &access.current,
            )?;
            if !world
                .executable_resources
                .contains(&process.executable_handle)
            {
                return invalid("Managed workspace process binding is not in the execution world.");
            }
        }
        let mut projected = resources
            .values()
            .map(|resource| resource.projection.clone())
            .collect::<Vec<_>>();
        projected.sort_by_key(|resource| resource.alias);
        let projection_ref = projection_ref(&access, &resources)?;
        let projection = WorkerWorkspaceProjectionV1 {
            schema_version: MANAGED_WORKSPACE_VERSION.into(),
            resources: projected,
            projection_ref,
        };
        Ok(Self {
            access,
            resources,
            process,
            projection,
        })
    }

    pub(crate) fn access(&self) -> &ManagedResourceAccessV1 {
        &self.access
    }

    pub(crate) fn projection(&self) -> WorkerWorkspaceProjectionV1 {
        self.projection.clone()
    }

    pub(crate) fn process(&self) -> Option<&ManagedWorkspaceProcessBindingV1> {
        self.process.as_ref()
    }

    pub(crate) fn resolve_process(
        &self,
        authority: &EffectAuthorityStateV1,
    ) -> AppResult<&ManagedWorkspaceProcessBindingV1> {
        let process = self.process.as_ref().ok_or_else(|| {
            AppError::InvalidInput("Managed workspace process binding is unavailable.".into())
        })?;
        let (world, _) = authority.validate_execution_world_attachment(
            &process.world_ref,
            &self.access.envelope_ref,
            &self.access.run_control_ref,
            &self.access.context,
            &self.access.current,
        )?;
        if !world
            .executable_resources
            .contains(&process.executable_handle)
        {
            return invalid("Managed workspace process binding changed or was substituted.");
        }
        Ok(process)
    }

    pub(crate) fn resolve(
        &self,
        authority: &EffectAuthorityStateV1,
        projection: &WorkerWorkspaceProjectionV1,
        alias: WorkerWorkspaceAliasV1,
        operation: WorkerWorkspaceOperationV1,
        relative_selector: &str,
    ) -> AppResult<ResourceHandleRefV1> {
        if projection != &self.projection {
            return invalid("Managed workspace projection is stale or belongs to another run.");
        }
        validate_managed_resource_selector(relative_selector)?;
        let resource = self.resources.get(&alias).ok_or_else(|| {
            AppError::InvalidInput("Managed workspace resource alias is unavailable.".into())
        })?;
        if !resource.projection.operations.contains(&operation)
            || (!resource.projection.relative_selectors && relative_selector != ".")
        {
            return invalid("Managed workspace projection does not expose that operation.");
        }
        let (current, effective_verbs) = authority.validate_resource_projection_attachment(
            &resource.grant.handle_ref,
            resource.grant.kind,
            &self.access.envelope_ref,
            &self.access.run_control_ref,
            &self.access.context,
            &self.access.current,
        )?;
        if current != resource.grant
            || effective_verbs != resource.effective_verbs
            || !effective_verbs.contains(&operation.resource_verb())
        {
            return invalid(
                "Managed workspace attachment changed or cannot perform that operation.",
            );
        }
        Ok(current.handle_ref)
    }
}

fn validate_alias_kind(alias: WorkerWorkspaceAliasV1, kind: ResourceKindV1) -> AppResult<()> {
    let valid = matches!(
        (alias, kind),
        (
            WorkerWorkspaceAliasV1::Input,
            ResourceKindV1::ManagedRevision
        ) | (WorkerWorkspaceAliasV1::Workspace, ResourceKindV1::Workspace)
            | (WorkerWorkspaceAliasV1::Output, ResourceKindV1::OutputSlot)
            | (WorkerWorkspaceAliasV1::Scratch, ResourceKindV1::Scratch)
    );
    if !valid {
        return invalid("Managed workspace alias and resource kind are mismatched.");
    }
    Ok(())
}

fn project_resource(
    alias: WorkerWorkspaceAliasV1,
    grant: &ResourceGrantV1,
    effective_verbs: &BTreeSet<ResourceVerbV1>,
) -> WorkerWorkspaceResourceProjectionV1 {
    let operations = [
        WorkerWorkspaceOperationV1::Inspect,
        WorkerWorkspaceOperationV1::Read,
        WorkerWorkspaceOperationV1::Create,
        WorkerWorkspaceOperationV1::Replace,
    ]
    .into_iter()
    .filter(|operation| effective_verbs.contains(&operation.resource_verb()))
    .collect();
    let kind = match grant.kind {
        ResourceKindV1::ManagedRevision => WorkerWorkspaceResourceKindV1::ManagedRevision,
        ResourceKindV1::Workspace => WorkerWorkspaceResourceKindV1::Workspace,
        ResourceKindV1::OutputSlot => WorkerWorkspaceResourceKindV1::Output,
        ResourceKindV1::Scratch => WorkerWorkspaceResourceKindV1::Scratch,
        _ => unreachable!("workspace alias validation excludes non-workspace resource kinds"),
    };
    WorkerWorkspaceResourceProjectionV1 {
        alias,
        kind,
        operations,
        relative_selectors: grant.kind != ResourceKindV1::ManagedRevision,
    }
}

fn projection_ref(
    access: &ManagedResourceAccessV1,
    resources: &BTreeMap<WorkerWorkspaceAliasV1, ManagedWorkspaceResourceLeaseV1>,
) -> AppResult<String> {
    let context_ref = access.context.context_ref()?;
    let attachments = resources
        .iter()
        .map(|(alias, resource)| {
            (
                *alias,
                resource.grant.kind,
                resource.grant.handle_ref.as_str(),
                resource.grant.selector_prefix.as_str(),
                &resource.grant.allowed_verbs,
                &resource.effective_verbs,
                resource.grant.expires_at,
            )
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&(
        MANAGED_WORKSPACE_VERSION,
        access.envelope_ref.as_str(),
        access.run_control_ref.as_str(),
        context_ref.as_str(),
        attachments,
    ))?;
    Ok(format!(
        "pastey-managed-workspace-projection-v1:{}",
        blake3::hash(&bytes).to_hex()
    ))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_output_workspace_and_scratch_aliases_are_kind_separated() {
        for (alias, kind) in [
            (
                WorkerWorkspaceAliasV1::Input,
                ResourceKindV1::ManagedRevision,
            ),
            (WorkerWorkspaceAliasV1::Workspace, ResourceKindV1::Workspace),
            (WorkerWorkspaceAliasV1::Output, ResourceKindV1::OutputSlot),
            (WorkerWorkspaceAliasV1::Scratch, ResourceKindV1::Scratch),
        ] {
            assert!(validate_alias_kind(alias, kind).is_ok());
        }
        assert!(
            validate_alias_kind(WorkerWorkspaceAliasV1::Input, ResourceKindV1::OutputSlot).is_err()
        );
        assert!(
            validate_alias_kind(WorkerWorkspaceAliasV1::Scratch, ResourceKindV1::Workspace)
                .is_err()
        );
        assert!(
            validate_alias_kind(WorkerWorkspaceAliasV1::Output, ResourceKindV1::Scratch).is_err()
        );
    }
}

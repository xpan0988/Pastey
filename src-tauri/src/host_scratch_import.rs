//! Host-authoritative complete Scratch-tree import.
//!
//! This is deliberately a Host resource mechanic: it re-reads a canonical
//! Scratch scan and imports bytes through ordinary EffectAuthority effects.
//! A backend must validate its own binding before calling it.

use crate::{
    effect_authority::{
        lower_tool_request, EffectAuthorityStateV1, EffectBudgetsV1, EffectDecisionV1,
        EffectRequestKindV1, ResourceEffectV1, ResourceVerbV1, StepWorkDescriptorV1,
        ToolEffectIntentV1, ToolRequestV1, EFFECT_AUTHORITY_VERSION,
    },
    error::{AppError, AppResult},
    managed_execution::ManagedStepGrantV1,
    managed_objects::ManagedObjectBindingService,
    managed_resources::{
        HostManagedResourceBackendV1, ManagedResourceAccessV1, ManagedResourceResolverV1,
        ManagedScratchLeaseV1, SealedOutputEvidenceV1,
    },
    safe_file_identity,
};

pub(crate) const MAX_HOST_SCRATCH_IMPORT_FILES: usize = 64;

pub(crate) struct HostScratchImportV1 {
    pub(crate) output_seal: SealedOutputEvidenceV1,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) evidence_head: String,
}

/// Canonically scans and imports the entire final Scratch tree. No backend
/// diff/claim influences the import, and any changed entry or partial effect
/// fails before a seal can be returned.
pub(crate) fn import_complete_scratch_to_output_slot(
    authority: &mut EffectAuthorityStateV1,
    resolver: &mut ManagedResourceResolverV1,
    objects: &mut ManagedObjectBindingService,
    grant: &ManagedStepGrantV1,
    access: &ManagedResourceAccessV1,
    scratch: &ManagedScratchLeaseV1,
    now: i64,
) -> AppResult<HostScratchImportV1> {
    if grant.operation != crate::effect_authority::ManagedSemanticOperationV1::Transform
        || grant.process_world.is_some()
        || grant.access.run_control_ref != access.run_control_ref
    {
        return invalid("Host Scratch import requires its exact private Transform claim.");
    }
    let output_slot = grant.output_slot.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Host Scratch import Transform has no OutputSlot.".into())
    })?;
    let scan = resolver.scan_specialist_scratch(authority, access, scratch)?;
    if scan.identity.files.len() > MAX_HOST_SCRATCH_IMPORT_FILES {
        return invalid("Host Scratch output exceeds the file limit.");
    }
    let first_sequence = authority.next_request_sequence(&access.run_control_ref)?;
    let intents = scan
        .identity
        .files
        .iter()
        .map(|(selector, identity)| ToolEffectIntentV1 {
            effect: EffectRequestKindV1::Resource(ResourceEffectV1 {
                verb: ResourceVerbV1::Create,
                handle_ref: output_slot.clone(),
                relative_selector: selector.clone(),
                value_digest: Some(identity.digest.clone()),
            }),
            requested_budget_slice: EffectBudgetsV1 {
                requests: 1,
                write_bytes: identity.byte_count,
                ..Default::default()
            },
            preconditions: vec![],
        })
        .collect();
    let requests = lower_tool_request(
        &StepWorkDescriptorV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            context: access.context.clone(),
            envelope_ref: access.envelope_ref.clone(),
            run_control_ref: access.run_control_ref.clone(),
            first_sequence,
        },
        &ToolRequestV1 {
            tool_name: "host-scratch-import-v1".into(),
            adapter_version_ref: "host-scratch-import-v1".into(),
            intents,
        },
    )?;
    let mut evidence = Vec::with_capacity(requests.len());
    for (request, (selector, identity)) in requests.iter().zip(&scan.identity.files) {
        let bytes = safe_file_identity::read_source_if_identity_matches(
            &scratch.root.join(selector),
            &scratch.root,
            identity,
            identity.byte_count,
        )?;
        resolver.stage_write_payload(authority, access, output_slot, &identity.digest, bytes)?;
        let mut backend = HostManagedResourceBackendV1::new(resolver, objects, now);
        let item = authority.enforce(request, &access.current, &mut backend)?;
        if item.decision != EffectDecisionV1::Allowed {
            return invalid("Host Scratch OutputSlot import effect was denied or unavailable.");
        }
        evidence.push(item);
    }
    if resolver
        .scan_specialist_scratch(authority, access, scratch)?
        .identity
        != scan.identity
    {
        return invalid("Scratch changed while the Host imported it.");
    }
    let output_seal = resolver.seal_output_slot(authority, access, output_slot, ".", &evidence)?;
    Ok(HostScratchImportV1 {
        output_seal,
        evidence_ids: evidence
            .iter()
            .map(|item| item.evidence_id.as_str().to_owned())
            .collect(),
        evidence_head: evidence
            .last()
            .ok_or_else(|| AppError::InvalidInput("Scratch final tree is empty.".into()))?
            .evidence_digest
            .clone(),
    })
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

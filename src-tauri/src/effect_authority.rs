//! Phase 5 generic Effect and Control Authority, runtime steps 1-7.
//!
//! This module is deliberately disconnected from live Bridge Plan dispatch.
//! It contains only pure contracts, process-local authority state, deterministic
//! lowering, and backend ports. Host-private managed resource resolution lives
//! in `managed_resources`; contained process effects live in `execution_world`;
//! independently scoped brokered network effects live in `network_broker`.
//! No live Bridge Plan dispatch is attached.

#![allow(dead_code)] // The generic authority surface is broader than the live Worker catalog.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    error::{AppError, AppResult},
    host_identity::{HostRef, HostSessionBinding, PlanParticipantRef},
};

pub(crate) const EFFECT_AUTHORITY_VERSION: &str = "pastey-effect-authority-v1";
const COMPILER_VERSION: &str = "pastey-effect-envelope-compiler-v1";
const MAX_ID_LEN: usize = 256;

macro_rules! opaque_ref {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }

            pub(crate) fn from_stored(value: String) -> AppResult<Self> {
                if value.trim().is_empty()
                    || value.len() > MAX_ID_LEN
                    || value.chars().any(char::is_control)
                {
                    return Err(AppError::InvalidInput(
                        "Stored effect authority reference is invalid.".into(),
                    ));
                }
                Ok(Self(value))
            }
        }
    };
}

opaque_ref!(AuthorityContextRefV1);
opaque_ref!(EffectEnvelopeRefV1);
opaque_ref!(ManagedRunRefV1);
opaque_ref!(ResourceHandleRefV1);
opaque_ref!(ExecutionWorldRefV1);
opaque_ref!(NetworkScopeRefV1);
opaque_ref!(EffectRequestIdV1);
opaque_ref!(EffectEvidenceIdV1);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedSemanticOperationV1 {
    Transform,
    Execute,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedInputRevisionV1 {
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
    pub(crate) host_ref: HostRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthorityContextV1 {
    pub(crate) contract_version: String,
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) approval_id: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) semantic_operation: ManagedSemanticOperationV1,
    pub(crate) participant_ref: PlanParticipantRef,
    pub(crate) host_ref: HostRef,
    pub(crate) admission_ref: String,
    pub(crate) session_binding_ref: String,
    pub(crate) input_revisions: Vec<ManagedInputRevisionV1>,
    pub(crate) issued_at: i64,
    pub(crate) expires_at: i64,
}

impl AuthorityContextV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        if self.contract_version != EFFECT_AUTHORITY_VERSION {
            return invalid("Unknown Phase 5 authority context version.");
        }
        for (value, label) in [
            (&self.bridge_id, "Bridge id"),
            (&self.plan_id, "Plan id"),
            (&self.revision_id, "revision id"),
            (&self.revision_hash, "revision hash"),
            (&self.approval_id, "approval id"),
            (&self.attempt_id, "attempt id"),
            (&self.step_id, "step id"),
            (&self.admission_ref, "admission ref"),
            (&self.session_binding_ref, "session binding ref"),
        ] {
            validate_id(value, label)?;
        }
        if self.issued_at <= 0 || self.expires_at <= self.issued_at {
            return invalid("Phase 5 authority context expiry is invalid.");
        }
        if PlanParticipantRef::for_host(&self.plan_id, &self.host_ref)? != self.participant_ref {
            return invalid("Phase 5 participant does not match the exact Plan and HostRef.");
        }
        if self.input_revisions.is_empty() {
            return invalid("Phase 5 authority requires exact input revisions.");
        }
        let mut inputs = BTreeSet::new();
        for input in &self.input_revisions {
            validate_id(&input.logical_object_id, "logical object id")?;
            if input.revision == 0 || input.host_ref != self.host_ref {
                return invalid("Phase 5 input revision has the wrong revision or Host location.");
            }
            if !inputs.insert(input.clone()) {
                return invalid("Phase 5 input revision is duplicated.");
            }
        }
        Ok(())
    }

    pub(crate) fn context_ref(&self) -> AppResult<AuthorityContextRefV1> {
        self.validate()?;
        Ok(AuthorityContextRefV1(domain_hash(
            "pastey-authority-context-v1",
            self,
        )?))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectBudgetsV1 {
    pub(crate) requests: u64,
    pub(crate) read_bytes: u64,
    pub(crate) write_bytes: u64,
    pub(crate) process_spawns: u64,
    pub(crate) process_signals: u64,
    pub(crate) cpu_millis: u64,
    pub(crate) memory_byte_millis: u64,
    pub(crate) wall_millis: u64,
    pub(crate) network_resolutions: u64,
    pub(crate) network_connections: u64,
    pub(crate) network_binds: u64,
    pub(crate) network_requests: u64,
    pub(crate) network_bytes: u64,
    pub(crate) network_time_millis: u64,
}

impl EffectBudgetsV1 {
    fn component_min(self, other: Self) -> Self {
        Self {
            requests: self.requests.min(other.requests),
            read_bytes: self.read_bytes.min(other.read_bytes),
            write_bytes: self.write_bytes.min(other.write_bytes),
            process_spawns: self.process_spawns.min(other.process_spawns),
            process_signals: self.process_signals.min(other.process_signals),
            cpu_millis: self.cpu_millis.min(other.cpu_millis),
            memory_byte_millis: self.memory_byte_millis.min(other.memory_byte_millis),
            wall_millis: self.wall_millis.min(other.wall_millis),
            network_resolutions: self.network_resolutions.min(other.network_resolutions),
            network_connections: self.network_connections.min(other.network_connections),
            network_binds: self.network_binds.min(other.network_binds),
            network_requests: self.network_requests.min(other.network_requests),
            network_bytes: self.network_bytes.min(other.network_bytes),
            network_time_millis: self.network_time_millis.min(other.network_time_millis),
        }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            requests: self.requests.checked_add(other.requests)?,
            read_bytes: self.read_bytes.checked_add(other.read_bytes)?,
            write_bytes: self.write_bytes.checked_add(other.write_bytes)?,
            process_spawns: self.process_spawns.checked_add(other.process_spawns)?,
            process_signals: self.process_signals.checked_add(other.process_signals)?,
            cpu_millis: self.cpu_millis.checked_add(other.cpu_millis)?,
            memory_byte_millis: self
                .memory_byte_millis
                .checked_add(other.memory_byte_millis)?,
            wall_millis: self.wall_millis.checked_add(other.wall_millis)?,
            network_resolutions: self
                .network_resolutions
                .checked_add(other.network_resolutions)?,
            network_connections: self
                .network_connections
                .checked_add(other.network_connections)?,
            network_binds: self.network_binds.checked_add(other.network_binds)?,
            network_requests: self.network_requests.checked_add(other.network_requests)?,
            network_bytes: self.network_bytes.checked_add(other.network_bytes)?,
            network_time_millis: self
                .network_time_millis
                .checked_add(other.network_time_millis)?,
        })
    }

    fn is_subset_of(&self, ceiling: &Self) -> bool {
        self.requests <= ceiling.requests
            && self.read_bytes <= ceiling.read_bytes
            && self.write_bytes <= ceiling.write_bytes
            && self.process_spawns <= ceiling.process_spawns
            && self.process_signals <= ceiling.process_signals
            && self.cpu_millis <= ceiling.cpu_millis
            && self.memory_byte_millis <= ceiling.memory_byte_millis
            && self.wall_millis <= ceiling.wall_millis
            && self.network_resolutions <= ceiling.network_resolutions
            && self.network_connections <= ceiling.network_connections
            && self.network_binds <= ceiling.network_binds
            && self.network_requests <= ceiling.network_requests
            && self.network_bytes <= ceiling.network_bytes
            && self.network_time_millis <= ceiling.network_time_millis
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceKindV1 {
    ManagedRevision,
    Workspace,
    OutputSlot,
    Scratch,
    Data,
    Secret,
    Executable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceVerbV1 {
    Inspect,
    Read,
    Create,
    Replace,
    Delete,
    SetMetadata,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProcessVerbV1 {
    Spawn,
    Signal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkVerbV1 {
    Resolve,
    Connect,
    Bind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResourceGrantV1 {
    pub(crate) handle_ref: ResourceHandleRefV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) host_ref: HostRef,
    pub(crate) kind: ResourceKindV1,
    pub(crate) safe_identity_ref: String,
    pub(crate) selector_prefix: String,
    pub(crate) allowed_verbs: BTreeSet<ResourceVerbV1>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfinementPropertyV1 {
    AuthorizedResourceProjection,
    AuthorityNeutralEnvironment,
    ExplicitProcessIo,
    PlatformSandboxedProcess,
    CancellableProcessSession,
    NoRawNetwork,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExecutionWorldGrantV1 {
    pub(crate) world_ref: ExecutionWorldRefV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) world_identity_digest: String,
    pub(crate) mounted_resources: BTreeSet<ResourceHandleRefV1>,
    pub(crate) executable_resources: BTreeSet<ResourceHandleRefV1>,
    pub(crate) required_properties: BTreeSet<ConfinementPropertyV1>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NetworkGrantV1 {
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) host_ref: HostRef,
    pub(crate) scope_refs: BTreeSet<NetworkScopeRefV1>,
    pub(crate) allowed_verbs: BTreeSet<NetworkVerbV1>,
    pub(crate) destination_refs: BTreeSet<String>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "authority", content = "grant")]
pub(crate) enum NetworkAuthorityV1 {
    Denied,
    Scoped(Box<NetworkGrantV1>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "family", content = "verb")]
pub(crate) enum EffectCapabilityV1 {
    Resource(ResourceVerbV1),
    Process(ProcessVerbV1),
    Network(NetworkVerbV1),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectBoundV1 {
    pub(crate) capability: EffectCapabilityV1,
    pub(crate) max_per_request: EffectBudgetsV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    tag = "resultKind",
    deny_unknown_fields
)]
pub(crate) enum ResultContractV1 {
    Transform {
        input: ManagedInputRevisionV1,
        output_revision: u64,
        output_slot: ResourceHandleRefV1,
    },
    Execute {
        inputs: Vec<ManagedInputRevisionV1>,
        result_schema_ref: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthorityCeilingV1 {
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) source_snapshot_ref: String,
    pub(crate) resources: Vec<ResourceGrantV1>,
    pub(crate) world: ExecutionWorldGrantV1,
    pub(crate) effect_bounds: Vec<EffectBoundV1>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) network: NetworkAuthorityV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectEnvelopeV1 {
    pub(crate) contract_version: String,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) compiler_version: String,
    pub(crate) host_policy_snapshot_ref: String,
    pub(crate) context: AuthorityContextV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) resources: Vec<ResourceGrantV1>,
    pub(crate) world: ExecutionWorldGrantV1,
    pub(crate) effect_bounds: Vec<EffectBoundV1>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) network: NetworkAuthorityV1,
    pub(crate) result_contract: ResultContractV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct EffectEnvelopeCompileRequestV1 {
    pub(crate) context: AuthorityContextV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) semantic_ceiling: AuthorityCeilingV1,
    pub(crate) admission_ceiling: AuthorityCeilingV1,
    pub(crate) host_policy_ceiling: AuthorityCeilingV1,
    pub(crate) confinement_ceiling: AuthorityCeilingV1,
    pub(crate) host_policy_snapshot_ref: String,
    pub(crate) result_contract: ResultContractV1,
}

pub(crate) fn compile_effect_envelope(
    request: EffectEnvelopeCompileRequestV1,
) -> AppResult<EffectEnvelopeV1> {
    request.context.validate()?;
    validate_id(
        &request.host_policy_snapshot_ref,
        "Host policy snapshot ref",
    )?;
    let context_ref = request.context.context_ref()?;
    let ceilings = [
        &request.semantic_ceiling,
        &request.admission_ceiling,
        &request.host_policy_ceiling,
        &request.confinement_ceiling,
    ];
    for ceiling in ceilings {
        validate_ceiling(
            ceiling,
            &context_ref,
            &request.run_control_ref,
            &request.context.host_ref,
        )?;
    }

    let resources = intersect_resources(&ceilings)?;
    let world = intersect_worlds(&ceilings)?;
    let effect_bounds = intersect_effect_bounds(&ceilings);
    let budgets = ceilings
        .iter()
        .skip(1)
        .fold(ceilings[0].budgets, |value, ceiling| {
            value.component_min(ceiling.budgets)
        });
    let network = intersect_network(&ceilings)?;
    let expires_at = ceilings
        .iter()
        .fold(request.context.expires_at, |value, ceiling| {
            value.min(ceiling.expires_at)
        });
    validate_result_contract(&request.context, &request.result_contract, &resources)?;

    let mut envelope = EffectEnvelopeV1 {
        contract_version: EFFECT_AUTHORITY_VERSION.into(),
        envelope_ref: EffectEnvelopeRefV1(String::new()),
        compiler_version: COMPILER_VERSION.into(),
        host_policy_snapshot_ref: request.host_policy_snapshot_ref,
        context: request.context,
        context_ref,
        run_control_ref: request.run_control_ref,
        resources,
        world,
        effect_bounds,
        budgets,
        network,
        result_contract: request.result_contract,
        expires_at,
    };
    envelope.envelope_ref = compute_envelope_ref(&envelope)?;
    for ceiling in ceilings {
        validate_envelope_subset(&envelope, ceiling)?;
    }
    Ok(envelope)
}

fn validate_ceiling(
    ceiling: &AuthorityCeilingV1,
    context_ref: &AuthorityContextRefV1,
    run_ref: &ManagedRunRefV1,
    host_ref: &HostRef,
) -> AppResult<()> {
    if &ceiling.context_ref != context_ref || ceiling.expires_at <= 0 {
        return invalid("Authority ceiling context or expiry is invalid.");
    }
    validate_id(&ceiling.source_snapshot_ref, "authority ceiling snapshot")?;
    let mut handles = HashSet::new();
    for grant in &ceiling.resources {
        if &grant.context_ref != context_ref
            || &grant.run_control_ref != run_ref
            || grant.expires_at <= 0
            || !handles.insert(grant.handle_ref.clone())
        {
            return invalid("Authority ceiling contains an invalid resource grant.");
        }
        validate_id(&grant.safe_identity_ref, "safe identity ref")?;
        validate_selector_prefix(&grant.selector_prefix)?;
        if grant.kind == ResourceKindV1::ManagedRevision
            && grant
                .allowed_verbs
                .iter()
                .any(|verb| !matches!(verb, ResourceVerbV1::Inspect | ResourceVerbV1::Read))
        {
            return invalid("Managed revision grants cannot contain mutation authority.");
        }
    }
    if ceiling.world.context_ref != *context_ref
        || ceiling.world.run_control_ref != *run_ref
        || ceiling.world.expires_at <= 0
    {
        return invalid("Authority ceiling execution world is invalid.");
    }
    validate_id(
        &ceiling.world.world_identity_digest,
        "execution world identity",
    )?;
    let mut capabilities = HashSet::new();
    for bound in &ceiling.effect_bounds {
        if !capabilities.insert(bound.capability) {
            return invalid("Authority ceiling repeats an effect capability.");
        }
    }
    if let NetworkAuthorityV1::Scoped(grant) = &ceiling.network {
        if grant.context_ref != *context_ref
            || grant.run_control_ref != *run_ref
            || grant.host_ref != *host_ref
            || grant.expires_at <= 0
            || grant.scope_refs.is_empty()
            || grant.allowed_verbs.is_empty()
            || grant.destination_refs.is_empty()
        {
            return invalid("Authority ceiling network grant is invalid.");
        }
        for destination in &grant.destination_refs {
            validate_id(destination, "network destination ref")?;
        }
    }
    Ok(())
}

fn intersect_resources(ceilings: &[&AuthorityCeilingV1; 4]) -> AppResult<Vec<ResourceGrantV1>> {
    let maps = ceilings.map(|ceiling| {
        ceiling
            .resources
            .iter()
            .map(|grant| (grant.handle_ref.clone(), grant))
            .collect::<BTreeMap<_, _>>()
    });
    let mut resources = Vec::new();
    for (handle_ref, first) in &maps[0] {
        let Some(grants) = maps[1..]
            .iter()
            .map(|map| map.get(handle_ref).copied())
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let mut all = vec![*first];
        all.extend(grants);
        if all.iter().any(|grant| {
            grant.context_ref != first.context_ref
                || grant.run_control_ref != first.run_control_ref
                || grant.host_ref != first.host_ref
                || grant.kind != first.kind
                || grant.safe_identity_ref != first.safe_identity_ref
                || grant.selector_prefix != first.selector_prefix
        }) {
            return invalid("Conflicting resource identity appeared across authority ceilings.");
        }
        let allowed_verbs = all
            .iter()
            .skip(1)
            .fold(first.allowed_verbs.clone(), |set, grant| {
                set.intersection(&grant.allowed_verbs).copied().collect()
            });
        if allowed_verbs.is_empty() {
            continue;
        }
        resources.push(ResourceGrantV1 {
            handle_ref: handle_ref.clone(),
            context_ref: first.context_ref.clone(),
            run_control_ref: first.run_control_ref.clone(),
            host_ref: first.host_ref.clone(),
            kind: first.kind,
            safe_identity_ref: first.safe_identity_ref.clone(),
            selector_prefix: first.selector_prefix.clone(),
            allowed_verbs,
            budgets: all.iter().skip(1).fold(first.budgets, |value, grant| {
                value.component_min(grant.budgets)
            }),
            expires_at: all
                .iter()
                .fold(first.expires_at, |value, grant| value.min(grant.expires_at)),
        });
    }
    resources.sort_by(|left, right| left.handle_ref.cmp(&right.handle_ref));
    Ok(resources)
}

fn intersect_worlds(ceilings: &[&AuthorityCeilingV1; 4]) -> AppResult<ExecutionWorldGrantV1> {
    let first = &ceilings[0].world;
    if ceilings[1..].iter().any(|ceiling| {
        ceiling.world.world_ref != first.world_ref
            || ceiling.world.context_ref != first.context_ref
            || ceiling.world.run_control_ref != first.run_control_ref
            || ceiling.world.world_identity_digest != first.world_identity_digest
    }) {
        return invalid("Execution world identity conflicts across authority ceilings.");
    }
    Ok(ExecutionWorldGrantV1 {
        world_ref: first.world_ref.clone(),
        context_ref: first.context_ref.clone(),
        run_control_ref: first.run_control_ref.clone(),
        world_identity_digest: first.world_identity_digest.clone(),
        mounted_resources: ceilings[1..].iter().fold(
            first.mounted_resources.clone(),
            |set, ceiling| {
                set.intersection(&ceiling.world.mounted_resources)
                    .cloned()
                    .collect()
            },
        ),
        executable_resources: ceilings[1..].iter().fold(
            first.executable_resources.clone(),
            |set, ceiling| {
                set.intersection(&ceiling.world.executable_resources)
                    .cloned()
                    .collect()
            },
        ),
        // More mandatory confinement properties reduce authority.
        required_properties: ceilings[1..].iter().fold(
            first.required_properties.clone(),
            |mut set, ceiling| {
                set.extend(ceiling.world.required_properties.iter().copied());
                set
            },
        ),
        budgets: ceilings[1..].iter().fold(first.budgets, |value, ceiling| {
            value.component_min(ceiling.world.budgets)
        }),
        expires_at: ceilings[1..]
            .iter()
            .fold(first.expires_at, |value, ceiling| {
                value.min(ceiling.world.expires_at)
            }),
    })
}

fn intersect_effect_bounds(ceilings: &[&AuthorityCeilingV1; 4]) -> Vec<EffectBoundV1> {
    let maps = ceilings.map(|ceiling| {
        ceiling
            .effect_bounds
            .iter()
            .map(|bound| (bound.capability, bound.max_per_request))
            .collect::<BTreeMap<_, _>>()
    });
    maps[0]
        .iter()
        .filter_map(|(capability, budget)| {
            let others = maps[1..]
                .iter()
                .map(|map| map.get(capability).copied())
                .collect::<Option<Vec<_>>>()?;
            Some(EffectBoundV1 {
                capability: *capability,
                max_per_request: others
                    .into_iter()
                    .fold(*budget, EffectBudgetsV1::component_min),
            })
        })
        .collect()
}

fn intersect_network(ceilings: &[&AuthorityCeilingV1; 4]) -> AppResult<NetworkAuthorityV1> {
    let grants = ceilings
        .iter()
        .map(|ceiling| match &ceiling.network {
            NetworkAuthorityV1::Denied => None,
            NetworkAuthorityV1::Scoped(grant) => Some(grant),
        })
        .collect::<Option<Vec<_>>>();
    let Some(grants) = grants else {
        return Ok(NetworkAuthorityV1::Denied);
    };
    let first = grants[0];
    if grants.iter().any(|grant| {
        grant.context_ref != first.context_ref
            || grant.run_control_ref != first.run_control_ref
            || grant.host_ref != first.host_ref
    }) {
        return invalid("Network authority context conflicts across ceilings.");
    }
    let scope_refs = grants[1..]
        .iter()
        .fold(first.scope_refs.clone(), |set, grant| {
            set.intersection(&grant.scope_refs).cloned().collect()
        });
    let allowed_verbs = grants[1..]
        .iter()
        .fold(first.allowed_verbs.clone(), |set, grant| {
            set.intersection(&grant.allowed_verbs).copied().collect()
        });
    let destination_refs = grants[1..]
        .iter()
        .fold(first.destination_refs.clone(), |set, grant| {
            set.intersection(&grant.destination_refs).cloned().collect()
        });
    if scope_refs.is_empty() || allowed_verbs.is_empty() || destination_refs.is_empty() {
        return Ok(NetworkAuthorityV1::Denied);
    }
    Ok(NetworkAuthorityV1::Scoped(Box::new(NetworkGrantV1 {
        context_ref: first.context_ref.clone(),
        run_control_ref: first.run_control_ref.clone(),
        host_ref: first.host_ref.clone(),
        scope_refs,
        allowed_verbs,
        destination_refs,
        budgets: grants[1..].iter().fold(first.budgets, |value, grant| {
            value.component_min(grant.budgets)
        }),
        expires_at: grants[1..]
            .iter()
            .fold(first.expires_at, |value, grant| value.min(grant.expires_at)),
    })))
}

pub(crate) fn validate_envelope_subset(
    envelope: &EffectEnvelopeV1,
    ceiling: &AuthorityCeilingV1,
) -> AppResult<()> {
    if envelope.context_ref != ceiling.context_ref
        || envelope.expires_at > ceiling.expires_at
        || !envelope.budgets.is_subset_of(&ceiling.budgets)
    {
        return invalid("Effect envelope widens its authority ceiling.");
    }
    let resources = ceiling
        .resources
        .iter()
        .map(|grant| (&grant.handle_ref, grant))
        .collect::<HashMap<_, _>>();
    for grant in &envelope.resources {
        let Some(limit) = resources.get(&grant.handle_ref) else {
            return invalid("Effect envelope adds a resource outside its ceiling.");
        };
        if grant.context_ref != limit.context_ref
            || grant.run_control_ref != limit.run_control_ref
            || grant.host_ref != limit.host_ref
            || grant.kind != limit.kind
            || grant.safe_identity_ref != limit.safe_identity_ref
            || grant.selector_prefix != limit.selector_prefix
            || !grant.allowed_verbs.is_subset(&limit.allowed_verbs)
            || !grant.budgets.is_subset_of(&limit.budgets)
            || grant.expires_at > limit.expires_at
        {
            return invalid("Effect envelope widens a resource grant.");
        }
    }
    if envelope.world.world_ref != ceiling.world.world_ref
        || envelope.world.world_identity_digest != ceiling.world.world_identity_digest
        || !envelope
            .world
            .mounted_resources
            .is_subset(&ceiling.world.mounted_resources)
        || !envelope
            .world
            .executable_resources
            .is_subset(&ceiling.world.executable_resources)
        || !envelope
            .world
            .required_properties
            .is_superset(&ceiling.world.required_properties)
        || !envelope.world.budgets.is_subset_of(&ceiling.world.budgets)
        || envelope.world.expires_at > ceiling.world.expires_at
    {
        return invalid("Effect envelope widens its execution world.");
    }
    let bounds = ceiling
        .effect_bounds
        .iter()
        .map(|bound| (bound.capability, bound.max_per_request))
        .collect::<HashMap<_, _>>();
    for bound in &envelope.effect_bounds {
        if !bounds
            .get(&bound.capability)
            .is_some_and(|limit| bound.max_per_request.is_subset_of(limit))
        {
            return invalid("Effect envelope adds or widens an effect capability.");
        }
    }
    if !network_is_subset(&envelope.network, &ceiling.network) {
        return invalid("Effect envelope widens independent network authority.");
    }
    Ok(())
}

fn network_is_subset(value: &NetworkAuthorityV1, ceiling: &NetworkAuthorityV1) -> bool {
    match (value, ceiling) {
        (NetworkAuthorityV1::Denied, _) => true,
        (NetworkAuthorityV1::Scoped(_), NetworkAuthorityV1::Denied) => false,
        (NetworkAuthorityV1::Scoped(value), NetworkAuthorityV1::Scoped(ceiling)) => {
            value.context_ref == ceiling.context_ref
                && value.run_control_ref == ceiling.run_control_ref
                && value.host_ref == ceiling.host_ref
                && value.scope_refs.is_subset(&ceiling.scope_refs)
                && value.allowed_verbs.is_subset(&ceiling.allowed_verbs)
                && value.destination_refs.is_subset(&ceiling.destination_refs)
                && value.budgets.is_subset_of(&ceiling.budgets)
                && value.expires_at <= ceiling.expires_at
        }
    }
}

fn validate_result_contract(
    context: &AuthorityContextV1,
    result: &ResultContractV1,
    resources: &[ResourceGrantV1],
) -> AppResult<()> {
    match (context.semantic_operation, result) {
        (
            ManagedSemanticOperationV1::Transform,
            ResultContractV1::Transform {
                input,
                output_revision,
                output_slot,
            },
        ) => {
            if !context.input_revisions.contains(input)
                || *output_revision != input.revision.checked_add(1).unwrap_or(0)
                || !resources.iter().any(|grant| {
                    grant.handle_ref == *output_slot
                        && grant.kind == ResourceKindV1::OutputSlot
                        && (grant.allowed_verbs.contains(&ResourceVerbV1::Create)
                            || grant.allowed_verbs.contains(&ResourceVerbV1::Replace))
                })
            {
                return invalid("Transform result contract widens or mismatches exact lineage.");
            }
        }
        (
            ManagedSemanticOperationV1::Execute,
            ResultContractV1::Execute {
                inputs,
                result_schema_ref,
            },
        ) => {
            validate_id(result_schema_ref, "Execute result schema ref")?;
            if inputs != &context.input_revisions {
                return invalid("Execute result contract does not match exact inputs.");
            }
        }
        _ => return invalid("Result contract does not match the semantic operation."),
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvelopeHashMaterial<'a> {
    contract_version: &'a str,
    compiler_version: &'a str,
    host_policy_snapshot_ref: &'a str,
    context: &'a AuthorityContextV1,
    context_ref: &'a AuthorityContextRefV1,
    run_control_ref: &'a ManagedRunRefV1,
    resources: &'a [ResourceGrantV1],
    world: &'a ExecutionWorldGrantV1,
    effect_bounds: &'a [EffectBoundV1],
    budgets: &'a EffectBudgetsV1,
    network: &'a NetworkAuthorityV1,
    result_contract: &'a ResultContractV1,
    expires_at: i64,
}

fn compute_envelope_ref(envelope: &EffectEnvelopeV1) -> AppResult<EffectEnvelopeRefV1> {
    Ok(EffectEnvelopeRefV1(domain_hash(
        "pastey-effect-envelope-v1",
        &EnvelopeHashMaterial {
            contract_version: &envelope.contract_version,
            compiler_version: &envelope.compiler_version,
            host_policy_snapshot_ref: &envelope.host_policy_snapshot_ref,
            context: &envelope.context,
            context_ref: &envelope.context_ref,
            run_control_ref: &envelope.run_control_ref,
            resources: &envelope.resources,
            world: &envelope.world,
            effect_bounds: &envelope.effect_bounds,
            budgets: &envelope.budgets,
            network: &envelope.network,
            result_contract: &envelope.result_contract,
            expires_at: envelope.expires_at,
        },
    )?))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResourceEffectV1 {
    pub(crate) verb: ResourceVerbV1,
    pub(crate) handle_ref: ResourceHandleRefV1,
    pub(crate) relative_selector: String,
    pub(crate) value_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    tag = "verb",
    deny_unknown_fields
)]
pub(crate) enum ProcessEffectV1 {
    Spawn {
        world_ref: ExecutionWorldRefV1,
        executable_handle: ResourceHandleRefV1,
        argv_digest: String,
        working_directory_handle: Option<ResourceHandleRefV1>,
        working_directory_selector: Option<String>,
        environment_digest: String,
        stdin_digest: Option<String>,
    },
    Signal {
        world_ref: ExecutionWorldRefV1,
        process_ref: String,
        signal_ref: String,
    },
}

impl ProcessEffectV1 {
    fn verb(&self) -> ProcessVerbV1 {
        match self {
            Self::Spawn { .. } => ProcessVerbV1::Spawn,
            Self::Signal { .. } => ProcessVerbV1::Signal,
        }
    }

    fn world_ref(&self) -> &ExecutionWorldRefV1 {
        match self {
            Self::Spawn { world_ref, .. } | Self::Signal { world_ref, .. } => world_ref,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NetworkEffectV1 {
    pub(crate) verb: NetworkVerbV1,
    pub(crate) scope_ref: NetworkScopeRefV1,
    pub(crate) destination_ref: String,
    pub(crate) transport_ref: String,
    /// Required for hostname connect after a brokered resolve. Literal IP
    /// destinations leave this absent.
    pub(crate) resolution_generation_ref: Option<String>,
    /// Digest of optional Host-brokered request bytes staged out-of-band.
    pub(crate) request_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    tag = "family",
    content = "effect",
    deny_unknown_fields
)]
pub(crate) enum EffectRequestKindV1 {
    Resource(ResourceEffectV1),
    Process(ProcessEffectV1),
    Network(NetworkEffectV1),
}

impl EffectRequestKindV1 {
    fn capability(&self) -> EffectCapabilityV1 {
        match self {
            Self::Resource(effect) => EffectCapabilityV1::Resource(effect.verb),
            Self::Process(effect) => EffectCapabilityV1::Process(effect.verb()),
            Self::Network(effect) => EffectCapabilityV1::Network(effect.verb),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    tag = "precondition",
    deny_unknown_fields
)]
pub(crate) enum EffectPreconditionV1 {
    ResourceGeneration {
        handle_ref: ResourceHandleRefV1,
        generation: u64,
        digest: String,
    },
    ProcessState {
        process_ref: String,
        expected_state: String,
    },
    DestinationGeneration {
        scope_ref: NetworkScopeRefV1,
        generation_ref: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectRequestV1 {
    pub(crate) contract_version: String,
    pub(crate) request_id: EffectRequestIdV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context: AuthorityContextV1,
    pub(crate) sequence: u64,
    pub(crate) adapter_version_ref: String,
    pub(crate) effect: EffectRequestKindV1,
    pub(crate) requested_budget_slice: EffectBudgetsV1,
    pub(crate) preconditions: Vec<EffectPreconditionV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StepWorkDescriptorV1 {
    pub(crate) contract_version: String,
    pub(crate) context: AuthorityContextV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) first_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolEffectIntentV1 {
    pub(crate) effect: EffectRequestKindV1,
    pub(crate) requested_budget_slice: EffectBudgetsV1,
    pub(crate) preconditions: Vec<EffectPreconditionV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolRequestV1 {
    /// Diagnostic Harness vocabulary only. It is intentionally excluded from
    /// authority identity and never consulted during lowering.
    pub(crate) tool_name: String,
    pub(crate) adapter_version_ref: String,
    pub(crate) intents: Vec<ToolEffectIntentV1>,
}

/// Deterministic, side-effect-free lowering. The adapter accepts already typed
/// generic intents; it neither discovers capabilities nor grants authority.
pub(crate) fn lower_tool_request(
    descriptor: &StepWorkDescriptorV1,
    tool_request: &ToolRequestV1,
) -> AppResult<Vec<EffectRequestV1>> {
    if descriptor.contract_version != EFFECT_AUTHORITY_VERSION || tool_request.intents.is_empty() {
        return invalid("Tool lowering contract is unavailable or empty.");
    }
    descriptor.context.validate()?;
    validate_id(&tool_request.tool_name, "tool diagnostic name")?;
    validate_id(&tool_request.adapter_version_ref, "tool adapter version")?;
    let mut lowered = Vec::with_capacity(tool_request.intents.len());
    for (offset, intent) in tool_request.intents.iter().enumerate() {
        let sequence = descriptor
            .first_sequence
            .checked_add(offset as u64)
            .ok_or_else(|| AppError::InvalidInput("Effect request sequence overflowed.".into()))?;
        let mut request = EffectRequestV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            request_id: EffectRequestIdV1(String::new()),
            envelope_ref: descriptor.envelope_ref.clone(),
            run_control_ref: descriptor.run_control_ref.clone(),
            context: descriptor.context.clone(),
            sequence,
            adapter_version_ref: tool_request.adapter_version_ref.clone(),
            effect: intent.effect.clone(),
            requested_budget_slice: intent.requested_budget_slice,
            preconditions: intent.preconditions.clone(),
        };
        request.request_id = compute_request_id(&request)?;
        lowered.push(request);
    }
    Ok(lowered)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestHashMaterial<'a> {
    contract_version: &'a str,
    envelope_ref: &'a EffectEnvelopeRefV1,
    run_control_ref: &'a ManagedRunRefV1,
    context: &'a AuthorityContextV1,
    sequence: u64,
    adapter_version_ref: &'a str,
    effect: &'a EffectRequestKindV1,
    requested_budget_slice: &'a EffectBudgetsV1,
    preconditions: &'a [EffectPreconditionV1],
}

fn compute_request_id(request: &EffectRequestV1) -> AppResult<EffectRequestIdV1> {
    Ok(EffectRequestIdV1(domain_hash(
        "pastey-effect-request-v1",
        &RequestHashMaterial {
            contract_version: &request.contract_version,
            envelope_ref: &request.envelope_ref,
            run_control_ref: &request.run_control_ref,
            context: &request.context,
            sequence: request.sequence,
            adapter_version_ref: &request.adapter_version_ref,
            effect: &request.effect,
            requested_budget_slice: &request.requested_budget_slice,
            preconditions: &request.preconditions,
        },
    )?))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedRunStateV1 {
    Created,
    Active,
    Cancelling,
    Finished,
    Revoked,
    Interrupted,
}

impl ManagedRunStateV1 {
    fn terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Revoked | Self::Interrupted)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedRunControlV1 {
    pub(crate) contract_version: String,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) state: ManagedRunStateV1,
    pub(crate) next_request_sequence: u64,
    pub(crate) cumulative_budget_debits: EffectBudgetsV1,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedRunDraftV1 {
    pub(crate) context: AuthorityContextV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
}

pub(crate) fn execution_world_ref_for(
    draft: &ManagedRunDraftV1,
    world_identity_digest: &str,
) -> AppResult<ExecutionWorldRefV1> {
    validate_id(world_identity_digest, "execution world identity")?;
    Ok(ExecutionWorldRefV1(domain_hash(
        "pastey-execution-world-v1",
        &(
            draft.context_ref.as_str(),
            draft.run_control_ref.as_str(),
            world_identity_digest,
        ),
    )?))
}

/// Derives a process-local opaque scope identity from an exact run and a
/// Host-owned canonical scope descriptor. The descriptor itself never enters
/// a Worker request or the immutable Plan/wire contract.
pub(crate) fn network_scope_ref_for(
    draft: &ManagedRunDraftV1,
    canonical_scope_digest: &str,
) -> AppResult<NetworkScopeRefV1> {
    network_scope_ref_for_context(
        &draft.context_ref,
        &draft.run_control_ref,
        &draft.context.host_ref,
        canonical_scope_digest,
    )
}

pub(crate) fn network_scope_ref_for_context(
    context_ref: &AuthorityContextRefV1,
    run_control_ref: &ManagedRunRefV1,
    host_ref: &HostRef,
    canonical_scope_digest: &str,
) -> AppResult<NetworkScopeRefV1> {
    validate_id(canonical_scope_digest, "network scope descriptor")?;
    Ok(NetworkScopeRefV1(domain_hash(
        "pastey-network-scope-v1",
        &(
            context_ref.as_str(),
            run_control_ref.as_str(),
            host_ref.as_str(),
            canonical_scope_digest,
        ),
    )?))
}

pub(crate) fn network_destination_ref_for(
    draft: &ManagedRunDraftV1,
    canonical_destination_digest: &str,
) -> AppResult<String> {
    network_destination_ref_for_context(
        &draft.context_ref,
        &draft.run_control_ref,
        &draft.context.host_ref,
        canonical_destination_digest,
    )
}

pub(crate) fn network_destination_ref_for_context(
    context_ref: &AuthorityContextRefV1,
    run_control_ref: &ManagedRunRefV1,
    host_ref: &HostRef,
    canonical_destination_digest: &str,
) -> AppResult<String> {
    validate_id(
        canonical_destination_digest,
        "network destination descriptor",
    )?;
    domain_hash(
        "pastey-network-destination-v1",
        &(
            context_ref.as_str(),
            run_control_ref.as_str(),
            host_ref.as_str(),
            canonical_destination_digest,
        ),
    )
}

#[derive(Clone, Debug)]
struct ResourceAuthorityRecordV1 {
    grant: ResourceGrantV1,
    envelope_ref: EffectEnvelopeRefV1,
    revoked: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectIntentV1 {
    pub(crate) request_id: EffectRequestIdV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) sequence: u64,
    pub(crate) request_digest: String,
    pub(crate) claimed_at: i64,
    pub(crate) terminal_evidence_id: Option<EffectEvidenceIdV1>,
    pub(crate) indeterminate: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EffectDecisionV1 {
    Allowed,
    Denied,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    tag = "factFamily",
    deny_unknown_fields
)]
pub(crate) enum EffectFactsV1 {
    Resource {
        handle_ref: ResourceHandleRefV1,
        generation: u64,
        content_digest: String,
        bytes: u64,
    },
    Process {
        world_ref: ExecutionWorldRefV1,
        process_ref: String,
        state: String,
    },
    ContainedProcess {
        world_ref: ExecutionWorldRefV1,
        process_ref: String,
        world_identity_digest: String,
        executable_identity_ref: String,
        argv_digest: String,
        environment_digest: String,
        state: String,
        exit_code: Option<i32>,
        stdout_digest: String,
        stdout_bytes: u64,
        stderr_digest: String,
        stderr_bytes: u64,
        termination_requested: bool,
        network_denied: bool,
        resource_effect_digest: String,
    },
    Network {
        scope_ref: NetworkScopeRefV1,
        connection_ref: String,
        state: String,
    },
    BrokeredNetwork {
        scope_ref: NetworkScopeRefV1,
        destination_ref: String,
        action_ref: String,
        state: String,
        scope_kind: String,
        transport_ref: String,
        resolved_endpoint_refs: Vec<String>,
        resolution_generation_ref: Option<String>,
        local_endpoint_ref: Option<String>,
        bytes_sent: u64,
        bytes_received: u64,
        elapsed_millis: u64,
        dns_revalidated: bool,
        proxy_ref: Option<String>,
        redirects_followed: u64,
        closed: bool,
    },
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EffectEvidenceV1 {
    pub(crate) contract_version: String,
    pub(crate) evidence_id: EffectEvidenceIdV1,
    pub(crate) evidence_digest: String,
    pub(crate) prior_evidence_digest: Option<String>,
    pub(crate) request_id: EffectRequestIdV1,
    pub(crate) envelope_ref: EffectEnvelopeRefV1,
    pub(crate) run_control_ref: ManagedRunRefV1,
    pub(crate) context_ref: AuthorityContextRefV1,
    pub(crate) sequence: u64,
    pub(crate) decision: EffectDecisionV1,
    pub(crate) observed_preconditions: Vec<EffectPreconditionV1>,
    pub(crate) actual_effect_summary: String,
    pub(crate) budget_debit: EffectBudgetsV1,
    pub(crate) facts: EffectFactsV1,
    host_authenticator: String,
}

/// Core-facing immutable view of a run's Host-authenticated evidence. A
/// Worker may echo these opaque identities in a result proposal, but cannot
/// construct this snapshot or its Host authenticator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletionEvidenceV1 {
    pub(crate) envelope: EffectEnvelopeV1,
    pub(crate) run: ManagedRunControlV1,
    pub(crate) evidence: Vec<EffectEvidenceV1>,
    pub(crate) evidence_head: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BackendEffectOutcomeV1 {
    pub(crate) decision: EffectDecisionV1,
    pub(crate) actual_effect_summary: String,
    pub(crate) facts: EffectFactsV1,
}

#[derive(Clone, Debug)]
pub(crate) enum BackendApplyV1 {
    Completed(BackendEffectOutcomeV1),
    /// The write-ahead intent was claimed, but terminal outcome is ambiguous.
    LostAfterIntent,
}

pub(crate) trait HostEffectBackendV1 {
    fn apply(&mut self, request: &EffectRequestV1) -> BackendApplyV1;
}

/// The steps 1-4 unavailable backend remains the default for unattached effect
/// families. Steps 5-7 add separate exact resource, process, and network
/// backends; none falls back to direct Host access.
#[derive(Default)]
pub(crate) struct UnavailableProductionEffectBackendV1;

impl HostEffectBackendV1 for UnavailableProductionEffectBackendV1 {
    fn apply(&mut self, _request: &EffectRequestV1) -> BackendApplyV1 {
        BackendApplyV1::Completed(BackendEffectOutcomeV1 {
            decision: EffectDecisionV1::Unavailable,
            actual_effect_summary: "production_effect_backend_unavailable".into(),
            facts: EffectFactsV1::None,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CurrentHostAuthorityV1 {
    pub(crate) session_binding: HostSessionBinding,
    pub(crate) bridge_active: bool,
    pub(crate) burned: bool,
    pub(crate) disconnected: bool,
    pub(crate) restarted: bool,
    pub(crate) now: i64,
}

/// Process-local managed-effect authority state. Construction after restart creates an empty
/// store; no envelope, handle, run, replay claim, or evidence authority is
/// reconstructed from durable v1/v2 Plan records.
pub(crate) struct EffectAuthorityStateV1 {
    envelopes: HashMap<EffectEnvelopeRefV1, EffectEnvelopeV1>,
    runs: HashMap<ManagedRunRefV1, ManagedRunControlV1>,
    resources: HashMap<ResourceHandleRefV1, ResourceAuthorityRecordV1>,
    intents: HashMap<EffectRequestIdV1, EffectIntentV1>,
    evidence: HashMap<ManagedRunRefV1, Vec<EffectEvidenceV1>>,
    next_run_nonce: u64,
    next_handle_nonce: u64,
    evidence_key: [u8; 32],
}

impl Default for EffectAuthorityStateV1 {
    fn default() -> Self {
        let seed = uuid::Uuid::new_v4();
        let mut key = [0_u8; 32];
        key.copy_from_slice(blake3::hash(seed.as_bytes()).as_bytes());
        Self::with_evidence_key(key)
    }
}

impl EffectAuthorityStateV1 {
    fn with_evidence_key(evidence_key: [u8; 32]) -> Self {
        Self {
            envelopes: HashMap::new(),
            runs: HashMap::new(),
            resources: HashMap::new(),
            intents: HashMap::new(),
            evidence: HashMap::new(),
            next_run_nonce: 0,
            next_handle_nonce: 0,
            evidence_key,
        }
    }

    pub(crate) fn begin_run(
        &mut self,
        context: AuthorityContextV1,
    ) -> AppResult<ManagedRunDraftV1> {
        let context_ref = context.context_ref()?;
        self.next_run_nonce = self
            .next_run_nonce
            .checked_add(1)
            .ok_or_else(|| AppError::InvalidInput("Managed run identity overflowed.".into()))?;
        let run_control_ref = ManagedRunRefV1(domain_hash(
            "pastey-managed-run-v1",
            &(context_ref.as_str(), self.next_run_nonce),
        )?);
        Ok(ManagedRunDraftV1 {
            context,
            context_ref,
            run_control_ref,
        })
    }

    pub(crate) fn mint_resource_grant(
        &mut self,
        draft: &ManagedRunDraftV1,
        spec: ResourceGrantSpecV1,
    ) -> AppResult<ResourceGrantV1> {
        validate_id(&spec.safe_identity_ref, "safe identity ref")?;
        validate_selector_prefix(&spec.selector_prefix)?;
        if spec.host_ref != draft.context.host_ref
            || spec.expires_at > draft.context.expires_at
            || spec.expires_at <= draft.context.issued_at
            || spec.allowed_verbs.is_empty()
        {
            return invalid("Resource grant specification widens its run context.");
        }
        if spec.kind == ResourceKindV1::ManagedRevision
            && spec
                .allowed_verbs
                .iter()
                .any(|verb| !matches!(verb, ResourceVerbV1::Inspect | ResourceVerbV1::Read))
        {
            return invalid("Managed revision handles are immutable authority roots.");
        }
        self.next_handle_nonce = self
            .next_handle_nonce
            .checked_add(1)
            .ok_or_else(|| AppError::InvalidInput("Resource handle identity overflowed.".into()))?;
        let handle_ref = ResourceHandleRefV1(domain_hash(
            "pastey-resource-handle-v1",
            &(
                draft.context_ref.as_str(),
                draft.run_control_ref.as_str(),
                self.next_handle_nonce,
                spec.kind,
                &spec.safe_identity_ref,
            ),
        )?);
        Ok(ResourceGrantV1 {
            handle_ref,
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            host_ref: spec.host_ref,
            kind: spec.kind,
            safe_identity_ref: spec.safe_identity_ref,
            selector_prefix: spec.selector_prefix,
            allowed_verbs: spec.allowed_verbs,
            budgets: spec.budgets,
            expires_at: spec.expires_at,
        })
    }

    pub(crate) fn install_envelope(
        &mut self,
        draft: ManagedRunDraftV1,
        envelope: EffectEnvelopeV1,
    ) -> AppResult<()> {
        if envelope.contract_version != EFFECT_AUTHORITY_VERSION
            || envelope.context != draft.context
            || envelope.context_ref != draft.context_ref
            || envelope.run_control_ref != draft.run_control_ref
            || compute_envelope_ref(&envelope)? != envelope.envelope_ref
            || self.envelopes.contains_key(&envelope.envelope_ref)
            || self.runs.contains_key(&envelope.run_control_ref)
        {
            return invalid("Effect envelope installation is stale, forged, or duplicated.");
        }
        for grant in &envelope.resources {
            if self.resources.contains_key(&grant.handle_ref) {
                return invalid("Resource handle is already owned by another envelope.");
            }
        }
        for grant in &envelope.resources {
            self.resources.insert(
                grant.handle_ref.clone(),
                ResourceAuthorityRecordV1 {
                    grant: grant.clone(),
                    envelope_ref: envelope.envelope_ref.clone(),
                    revoked: false,
                },
            );
        }
        self.runs.insert(
            envelope.run_control_ref.clone(),
            ManagedRunControlV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                run_control_ref: envelope.run_control_ref.clone(),
                context_ref: envelope.context_ref.clone(),
                envelope_ref: envelope.envelope_ref.clone(),
                state: ManagedRunStateV1::Created,
                next_request_sequence: 0,
                cumulative_budget_debits: EffectBudgetsV1::default(),
                expires_at: envelope.expires_at,
            },
        );
        self.envelopes
            .insert(envelope.envelope_ref.clone(), envelope);
        Ok(())
    }

    pub(crate) fn activate_run(&mut self, run_ref: &ManagedRunRefV1, now: i64) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if run.state != ManagedRunStateV1::Created || now >= run.expires_at {
            return invalid("Only an unexpired created managed run may activate.");
        }
        run.state = ManagedRunStateV1::Active;
        Ok(())
    }

    pub(crate) fn cancel_run(&mut self, run_ref: &ManagedRunRefV1) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if run.state != ManagedRunStateV1::Active {
            return invalid("Only an active managed run may begin cancellation.");
        }
        run.state = ManagedRunStateV1::Cancelling;
        self.revoke_run_resources(run_ref);
        Ok(())
    }

    /// Host lifecycle cancellation may race the Harness's own error cleanup
    /// after the Host has already made the durable attempt terminal. Treat an
    /// already cancelling/terminal run as settled, while still rejecting an
    /// unknown or never-activated run.
    pub(crate) fn cancel_run_or_confirm_terminal(
        &mut self,
        run_ref: &ManagedRunRefV1,
    ) -> AppResult<()> {
        let state = self
            .runs
            .get(run_ref)
            .map(|run| run.state)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        match state {
            ManagedRunStateV1::Active => self.cancel_run(run_ref),
            ManagedRunStateV1::Cancelling
            | ManagedRunStateV1::Finished
            | ManagedRunStateV1::Revoked
            | ManagedRunStateV1::Interrupted => Ok(()),
            ManagedRunStateV1::Created => {
                invalid("Only an activated managed run may settle cancellation.")
            }
        }
    }

    pub(crate) fn finish_run(&mut self, run_ref: &ManagedRunRefV1) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if !matches!(
            run.state,
            ManagedRunStateV1::Active | ManagedRunStateV1::Cancelling
        ) {
            return invalid("Only an active or cancelling managed run may finish.");
        }
        run.state = ManagedRunStateV1::Finished;
        self.revoke_run_resources(run_ref);
        Ok(())
    }

    /// Core semantic completion is stricter than cleanup completion: a run
    /// already cancelling can never race into authoritative success.
    pub(crate) fn complete_run_authoritatively(
        &mut self,
        run_ref: &ManagedRunRefV1,
    ) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if run.state != ManagedRunStateV1::Active {
            return invalid("Only an active managed run may complete authoritatively.");
        }
        run.state = ManagedRunStateV1::Finished;
        self.revoke_run_resources(run_ref);
        Ok(())
    }

    pub(crate) fn run_refs_for_session(
        &self,
        session_binding_ref: &str,
    ) -> BTreeSet<ManagedRunRefV1> {
        self.envelopes
            .values()
            .filter(|envelope| envelope.context.session_binding_ref == session_binding_ref)
            .map(|envelope| envelope.run_control_ref.clone())
            .collect()
    }

    pub(crate) fn revoke_run(&mut self, run_ref: &ManagedRunRefV1) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if run.state.terminal() {
            return invalid("Terminal managed runs cannot be reopened or transitioned.");
        }
        run.state = ManagedRunStateV1::Revoked;
        self.revoke_run_resources(run_ref);
        Ok(())
    }

    fn interrupt_run(&mut self, run_ref: &ManagedRunRefV1) -> AppResult<()> {
        let run = self.run_mut(run_ref)?;
        if run.state.terminal() {
            return invalid("Terminal managed runs cannot be interrupted again.");
        }
        run.state = ManagedRunStateV1::Interrupted;
        self.revoke_run_resources(run_ref);
        Ok(())
    }

    pub(crate) fn revoke_bridge(&mut self, bridge_id: &str) {
        let runs = self
            .envelopes
            .values()
            .filter(|envelope| envelope.context.bridge_id == bridge_id)
            .map(|envelope| envelope.run_control_ref.clone())
            .collect::<Vec<_>>();
        self.revoke_many(&runs);
    }

    pub(crate) fn revoke_session(&mut self, session_binding_ref: &str) {
        let runs = self
            .envelopes
            .values()
            .filter(|envelope| envelope.context.session_binding_ref == session_binding_ref)
            .map(|envelope| envelope.run_control_ref.clone())
            .collect::<Vec<_>>();
        self.revoke_many(&runs);
    }

    pub(crate) fn revoke_all(&mut self) {
        let runs = self.runs.keys().cloned().collect::<Vec<_>>();
        self.revoke_many(&runs);
    }

    fn revoke_many(&mut self, run_refs: &[ManagedRunRefV1]) {
        for run_ref in run_refs {
            if let Some(run) = self.runs.get_mut(run_ref) {
                if !run.state.terminal() {
                    run.state = ManagedRunStateV1::Revoked;
                }
            }
            self.revoke_run_resources(run_ref);
        }
    }

    fn revoke_run_resources(&mut self, run_ref: &ManagedRunRefV1) {
        for record in self.resources.values_mut() {
            if record.grant.run_control_ref == *run_ref {
                record.revoked = true;
            }
        }
    }

    fn run_mut(&mut self, run_ref: &ManagedRunRefV1) -> AppResult<&mut ManagedRunControlV1> {
        self.runs
            .get_mut(run_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run authority is unavailable.".into()))
    }

    pub(crate) fn enforce<B: HostEffectBackendV1>(
        &mut self,
        request: &EffectRequestV1,
        current: &CurrentHostAuthorityV1,
        backend: &mut B,
    ) -> AppResult<EffectEvidenceV1> {
        self.validate_exact_request_context(request, current)?;
        let envelope = self
            .envelopes
            .get(&request.envelope_ref)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let run = self
            .runs
            .get(&request.run_control_ref)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        if run.state != ManagedRunStateV1::Active || current.now >= run.expires_at {
            if current.now >= run.expires_at && !run.state.terminal() {
                let _ = self.revoke_run(&request.run_control_ref);
            }
            return invalid("Managed run is not active and unexpired.");
        }
        if request.sequence != run.next_request_sequence
            || self.intents.contains_key(&request.request_id)
        {
            return invalid("Effect request sequence is replayed, skipped, or out of order.");
        }

        // Exact, ordered requests claim write-ahead intent before any backend
        // decision. Policy denials consume the sequence but not unspent budget.
        let request_digest = domain_hash("pastey-effect-request-intent-v1", request)?;
        self.intents.insert(
            request.request_id.clone(),
            EffectIntentV1 {
                request_id: request.request_id.clone(),
                envelope_ref: request.envelope_ref.clone(),
                run_control_ref: request.run_control_ref.clone(),
                sequence: request.sequence,
                request_digest,
                claimed_at: current.now,
                terminal_evidence_id: None,
                indeterminate: false,
            },
        );
        self.runs
            .get_mut(&request.run_control_ref)
            .expect("validated run")
            .next_request_sequence += 1;

        let authorization = self.authorize_effect(&envelope, request, current.now);
        let (decision, summary, facts, debit) = match authorization {
            Err(reason) => (
                EffectDecisionV1::Denied,
                reason,
                EffectFactsV1::None,
                EffectBudgetsV1::default(),
            ),
            Ok(()) => {
                let next = run
                    .cumulative_budget_debits
                    .checked_add(request.requested_budget_slice)
                    .filter(|value| value.is_subset_of(&envelope.budgets));
                let Some(next) = next else {
                    return self.append_denial_evidence(
                        &envelope,
                        request,
                        "cumulative_budget_exhausted",
                    );
                };
                // Reserve the complete requested slice atomically. Backend
                // denial/unavailability cannot reset it for a retry.
                self.runs
                    .get_mut(&request.run_control_ref)
                    .expect("validated run")
                    .cumulative_budget_debits = next;
                match backend.apply(request) {
                    BackendApplyV1::Completed(outcome) => (
                        outcome.decision,
                        outcome.actual_effect_summary,
                        outcome.facts,
                        request.requested_budget_slice,
                    ),
                    BackendApplyV1::LostAfterIntent => {
                        if let Some(intent) = self.intents.get_mut(&request.request_id) {
                            intent.indeterminate = true;
                        }
                        self.interrupt_run(&request.run_control_ref)?;
                        return invalid(
                            "Effect intent has no terminal evidence; run is indeterminate.",
                        );
                    }
                }
            }
        };
        self.append_evidence(&envelope, request, decision, summary, facts, debit)
    }

    fn validate_exact_request_context(
        &self,
        request: &EffectRequestV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<()> {
        if request.contract_version != EFFECT_AUTHORITY_VERSION
            || request.context.validate().is_err()
            || compute_request_id(request)? != request.request_id
        {
            return invalid("Effect request version, context, or identity is invalid.");
        }
        let envelope = self
            .envelopes
            .get(&request.envelope_ref)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let run = self
            .runs
            .get(&request.run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        if request.context != envelope.context
            || request.run_control_ref != envelope.run_control_ref
            || run.envelope_ref != request.envelope_ref
            || run.context_ref != envelope.context_ref
            || !current.bridge_active
            || current.burned
            || current.disconnected
            || current.restarted
            || current.now >= request.context.expires_at
            || current.session_binding.binding_ref != request.context.session_binding_ref
            || current.session_binding.bridge_id != request.context.bridge_id
            || current.session_binding.local_host_ref != request.context.host_ref
            || current.session_binding.expires_at <= current.now
        {
            return invalid("Effect request context, Host, session, or lifecycle is mismatched.");
        }
        Ok(())
    }

    /// Revalidates a Host-private resource attachment against the exact active
    /// envelope, run, semantic context, Host session, and lifecycle. This is
    /// intentionally narrower than effect authorization: attachment cannot
    /// grant a verb, budget, selector, or execution capability.
    pub(crate) fn validate_resource_attachment(
        &self,
        handle_ref: &ResourceHandleRefV1,
        expected_kind: ResourceKindV1,
        envelope_ref: &EffectEnvelopeRefV1,
        run_control_ref: &ManagedRunRefV1,
        context: &AuthorityContextV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<ResourceGrantV1> {
        let envelope = self
            .envelopes
            .get(envelope_ref)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let run = self
            .runs
            .get(run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        let record = self
            .resources
            .get(handle_ref)
            .ok_or_else(|| AppError::InvalidInput("Resource handle is unavailable.".into()))?;
        if context.validate().is_err()
            || context != &envelope.context
            || envelope.run_control_ref != *run_control_ref
            || run.envelope_ref != *envelope_ref
            || run.context_ref != envelope.context_ref
            || run.state != ManagedRunStateV1::Active
            || record.revoked
            || record.envelope_ref != *envelope_ref
            || record.grant.context_ref != envelope.context_ref
            || record.grant.run_control_ref != *run_control_ref
            || record.grant.host_ref != context.host_ref
            || record.grant.kind != expected_kind
            || record.grant.expires_at <= current.now
            || current.now >= run.expires_at
            || current.now >= context.expires_at
            || !current.bridge_active
            || current.burned
            || current.disconnected
            || current.restarted
            || current.session_binding.binding_ref != context.session_binding_ref
            || current.session_binding.bridge_id != context.bridge_id
            || current.session_binding.local_host_ref != context.host_ref
            || current.session_binding.expires_at <= current.now
        {
            return invalid(
                "Resource attachment context, Host, session, run, or lifecycle is mismatched.",
            );
        }
        Ok(record.grant.clone())
    }

    /// Read-only projection helper for Host-private adapters. It intersects an
    /// already validated resource grant with the envelope's existing effect
    /// bounds. The returned verbs are descriptive only and cannot authorize an
    /// effect or change either authority source.
    pub(crate) fn validate_resource_projection_attachment(
        &self,
        handle_ref: &ResourceHandleRefV1,
        expected_kind: ResourceKindV1,
        envelope_ref: &EffectEnvelopeRefV1,
        run_control_ref: &ManagedRunRefV1,
        context: &AuthorityContextV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<(ResourceGrantV1, BTreeSet<ResourceVerbV1>)> {
        let grant = self.validate_resource_attachment(
            handle_ref,
            expected_kind,
            envelope_ref,
            run_control_ref,
            context,
            current,
        )?;
        let envelope = self
            .envelopes
            .get(envelope_ref)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let bounded = envelope
            .effect_bounds
            .iter()
            .filter_map(|bound| match bound.capability {
                EffectCapabilityV1::Resource(verb) => Some(verb),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let effective = grant
            .allowed_verbs
            .intersection(&bounded)
            .copied()
            .collect();
        Ok((grant, effective))
    }

    /// Revalidates an exact process-local execution-world attachment. It
    /// returns only grants already present in the immutable envelope; it does
    /// not mint process, resource, or network authority.
    pub(crate) fn validate_execution_world_attachment(
        &self,
        world_ref: &ExecutionWorldRefV1,
        envelope_ref: &EffectEnvelopeRefV1,
        run_control_ref: &ManagedRunRefV1,
        context: &AuthorityContextV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<(ExecutionWorldGrantV1, Vec<ResourceGrantV1>)> {
        let envelope = self
            .envelopes
            .get(envelope_ref)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let run = self
            .runs
            .get(run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        if context.validate().is_err()
            || context != &envelope.context
            || envelope.run_control_ref != *run_control_ref
            || run.envelope_ref != *envelope_ref
            || run.context_ref != envelope.context_ref
            || run.state != ManagedRunStateV1::Active
            || envelope.world.world_ref != *world_ref
            || envelope.world.context_ref != envelope.context_ref
            || envelope.world.run_control_ref != *run_control_ref
            || envelope.world.expires_at <= current.now
            || !envelope
                .world
                .required_properties
                .contains(&ConfinementPropertyV1::NoRawNetwork)
            || current.now >= run.expires_at
            || current.now >= context.expires_at
            || !current.bridge_active
            || current.burned
            || current.disconnected
            || current.restarted
            || current.session_binding.binding_ref != context.session_binding_ref
            || current.session_binding.bridge_id != context.bridge_id
            || current.session_binding.local_host_ref != context.host_ref
            || current.session_binding.expires_at <= current.now
        {
            return invalid(
                "Execution world context, Host, session, network, run, or lifecycle is mismatched.",
            );
        }
        let handles = envelope
            .world
            .mounted_resources
            .union(&envelope.world.executable_resources)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut grants = Vec::with_capacity(handles.len());
        for handle in handles {
            let record = self.resources.get(&handle).ok_or_else(|| {
                AppError::InvalidInput("Execution world resource is unavailable.".into())
            })?;
            if record.revoked
                || record.envelope_ref != *envelope_ref
                || record.grant.context_ref != envelope.context_ref
                || record.grant.run_control_ref != *run_control_ref
                || record.grant.host_ref != context.host_ref
                || record.grant.expires_at <= current.now
            {
                return invalid("Execution world resource attachment is stale or mismatched.");
            }
            grants.push(record.grant.clone());
        }
        Ok((envelope.world.clone(), grants))
    }

    /// Revalidates a Host-private broker attachment. This returns only the
    /// immutable grant already present in the envelope and never resolves a
    /// destination, opens a socket, or widens network authority.
    pub(crate) fn validate_network_attachment(
        &self,
        scope_ref: &NetworkScopeRefV1,
        destination_ref: &str,
        envelope_ref: &EffectEnvelopeRefV1,
        run_control_ref: &ManagedRunRefV1,
        context: &AuthorityContextV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<NetworkGrantV1> {
        let envelope = self
            .envelopes
            .get(envelope_ref)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        let run = self
            .runs
            .get(run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        let NetworkAuthorityV1::Scoped(grant) = &envelope.network else {
            return invalid("Network authority is default-denied.");
        };
        if context.validate().is_err()
            || context != &envelope.context
            || envelope.run_control_ref != *run_control_ref
            || run.envelope_ref != *envelope_ref
            || run.context_ref != envelope.context_ref
            || run.state != ManagedRunStateV1::Active
            || grant.context_ref != envelope.context_ref
            || grant.run_control_ref != *run_control_ref
            || grant.host_ref != context.host_ref
            || !grant.scope_refs.contains(scope_ref)
            || !grant.destination_refs.contains(destination_ref)
            || grant.expires_at <= current.now
            || current.now >= run.expires_at
            || current.now >= context.expires_at
            || !current.bridge_active
            || current.burned
            || current.disconnected
            || current.restarted
            || current.session_binding.binding_ref != context.session_binding_ref
            || current.session_binding.bridge_id != context.bridge_id
            || current.session_binding.local_host_ref != context.host_ref
            || current.session_binding.expires_at <= current.now
        {
            return invalid(
                "Network attachment context, Host, session, run, destination, or lifecycle is mismatched.",
            );
        }
        Ok((**grant).clone())
    }

    pub(crate) fn validate_network_request_attachment(
        &self,
        request: &EffectRequestV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<NetworkGrantV1> {
        self.validate_exact_request_context(request, current)?;
        let run = self
            .runs
            .get(&request.run_control_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        if request.sequence != run.next_request_sequence
            || self.intents.contains_key(&request.request_id)
        {
            return invalid("Network request is replayed, skipped, or out of order.");
        }
        let EffectRequestKindV1::Network(effect) = &request.effect else {
            return invalid("Expected a brokered network request.");
        };
        self.validate_network_attachment(
            &effect.scope_ref,
            &effect.destination_ref,
            &request.envelope_ref,
            &request.run_control_ref,
            &request.context,
            current,
        )
    }

    pub(crate) fn validate_terminal_resource_evidence(
        &self,
        evidence: &EffectEvidenceV1,
        handle_ref: &ResourceHandleRefV1,
        envelope_ref: &EffectEnvelopeRefV1,
        run_control_ref: &ManagedRunRefV1,
        context_ref: &AuthorityContextRefV1,
    ) -> AppResult<()> {
        let chain = self.evidence.get(run_control_ref).ok_or_else(|| {
            AppError::InvalidInput("Managed resource evidence is unavailable.".into())
        })?;
        let facts_match = matches!(
            &evidence.facts,
            EffectFactsV1::Resource { handle_ref: observed, .. } if observed == handle_ref
        );
        if evidence.decision != EffectDecisionV1::Allowed
            || evidence.envelope_ref != *envelope_ref
            || evidence.run_control_ref != *run_control_ref
            || evidence.context_ref != *context_ref
            || !facts_match
            || !chain.iter().any(|stored| stored == evidence)
            || compute_evidence_id(evidence)? != evidence.evidence_id
            || compute_evidence_digest(evidence)? != evidence.evidence_digest
            || host_authenticator(&self.evidence_key, evidence)? != evidence.host_authenticator
        {
            return invalid("Managed resource evidence is stale, forged, or mismatched.");
        }
        Ok(())
    }

    fn authorize_effect(
        &self,
        envelope: &EffectEnvelopeV1,
        request: &EffectRequestV1,
        now: i64,
    ) -> Result<(), String> {
        let Some(bound) = envelope
            .effect_bounds
            .iter()
            .find(|bound| bound.capability == request.effect.capability())
        else {
            return Err("effect_capability_denied".into());
        };
        if !request
            .requested_budget_slice
            .is_subset_of(&bound.max_per_request)
            || request.requested_budget_slice.requests != 1
        {
            return Err("per_request_budget_denied".into());
        }
        match &request.effect {
            EffectRequestKindV1::Resource(effect) => {
                validate_relative_selector(&effect.relative_selector)
                    .map_err(|_| "resource_selector_denied".to_string())?;
                let record = self
                    .resources
                    .get(&effect.handle_ref)
                    .ok_or_else(|| "resource_handle_unavailable".to_string())?;
                if record.revoked
                    || record.envelope_ref != envelope.envelope_ref
                    || record.grant.run_control_ref != request.run_control_ref
                    || record.grant.context_ref != envelope.context_ref
                    || record.grant.host_ref != envelope.context.host_ref
                    || record.grant.expires_at <= now
                    || !record.grant.allowed_verbs.contains(&effect.verb)
                    || !request
                        .requested_budget_slice
                        .is_subset_of(&record.grant.budgets)
                    || !selector_within_prefix(
                        &effect.relative_selector,
                        &record.grant.selector_prefix,
                    )
                {
                    return Err("resource_grant_denied".into());
                }
            }
            EffectRequestKindV1::Process(effect) => {
                if effect.world_ref() != &envelope.world.world_ref
                    || envelope.world.context_ref != envelope.context_ref
                    || envelope.world.run_control_ref != request.run_control_ref
                    || envelope.world.expires_at <= now
                    || !request
                        .requested_budget_slice
                        .is_subset_of(&envelope.world.budgets)
                {
                    return Err("execution_world_denied".into());
                }
                if let ProcessEffectV1::Spawn {
                    executable_handle,
                    working_directory_handle,
                    working_directory_selector,
                    ..
                } = effect
                {
                    if !envelope
                        .world
                        .executable_resources
                        .contains(executable_handle)
                        || !self.handle_belongs_to_envelope(executable_handle, envelope)
                    {
                        return Err("executable_handle_denied".into());
                    }
                    if let Some(handle) = working_directory_handle {
                        if !envelope.world.mounted_resources.contains(handle)
                            || !self.handle_belongs_to_envelope(handle, envelope)
                        {
                            return Err("working_directory_handle_denied".into());
                        }
                    }
                    if let Some(selector) = working_directory_selector {
                        validate_relative_selector(selector)
                            .map_err(|_| "working_directory_selector_denied".to_string())?;
                    }
                }
            }
            EffectRequestKindV1::Network(effect) => {
                let NetworkAuthorityV1::Scoped(grant) = &envelope.network else {
                    return Err("network_default_denied".into());
                };
                if validate_id(&effect.destination_ref, "network destination ref").is_err()
                    || validate_id(&effect.transport_ref, "network transport ref").is_err()
                    || effect
                        .resolution_generation_ref
                        .as_deref()
                        .is_some_and(|value| {
                            validate_id(value, "network resolution generation").is_err()
                        })
                    || effect
                        .request_digest
                        .as_deref()
                        .is_some_and(|value| validate_id(value, "network request digest").is_err())
                {
                    return Err("network_request_malformed".into());
                }
                if grant.context_ref != envelope.context_ref
                    || grant.run_control_ref != request.run_control_ref
                    || grant.host_ref != envelope.context.host_ref
                    || grant.expires_at <= now
                    || !grant.scope_refs.contains(&effect.scope_ref)
                    || !grant.allowed_verbs.contains(&effect.verb)
                    || !grant.destination_refs.contains(&effect.destination_ref)
                    || !request.requested_budget_slice.is_subset_of(&grant.budgets)
                {
                    return Err("network_scope_denied".into());
                }
            }
        }
        Ok(())
    }

    fn handle_belongs_to_envelope(
        &self,
        handle: &ResourceHandleRefV1,
        envelope: &EffectEnvelopeV1,
    ) -> bool {
        self.resources.get(handle).is_some_and(|record| {
            !record.revoked
                && record.envelope_ref == envelope.envelope_ref
                && record.grant.run_control_ref == envelope.run_control_ref
                && record.grant.context_ref == envelope.context_ref
        })
    }

    fn append_denial_evidence(
        &mut self,
        envelope: &EffectEnvelopeV1,
        request: &EffectRequestV1,
        summary: &str,
    ) -> AppResult<EffectEvidenceV1> {
        self.append_evidence(
            envelope,
            request,
            EffectDecisionV1::Denied,
            summary.into(),
            EffectFactsV1::None,
            EffectBudgetsV1::default(),
        )
    }

    fn append_evidence(
        &mut self,
        envelope: &EffectEnvelopeV1,
        request: &EffectRequestV1,
        decision: EffectDecisionV1,
        summary: String,
        facts: EffectFactsV1,
        budget_debit: EffectBudgetsV1,
    ) -> AppResult<EffectEvidenceV1> {
        validate_id(&summary, "effect evidence summary")?;
        let prior_evidence_digest = self
            .evidence
            .get(&request.run_control_ref)
            .and_then(|items| items.last())
            .map(|item| item.evidence_digest.clone());
        let mut evidence = EffectEvidenceV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            evidence_id: EffectEvidenceIdV1(String::new()),
            evidence_digest: String::new(),
            prior_evidence_digest,
            request_id: request.request_id.clone(),
            envelope_ref: request.envelope_ref.clone(),
            run_control_ref: request.run_control_ref.clone(),
            context_ref: envelope.context_ref.clone(),
            sequence: request.sequence,
            decision,
            observed_preconditions: request.preconditions.clone(),
            actual_effect_summary: summary,
            budget_debit,
            facts,
            host_authenticator: String::new(),
        };
        evidence.evidence_id = compute_evidence_id(&evidence)?;
        evidence.evidence_digest = compute_evidence_digest(&evidence)?;
        evidence.host_authenticator = host_authenticator(&self.evidence_key, &evidence)?;
        self.intents
            .get_mut(&request.request_id)
            .expect("write-ahead intent")
            .terminal_evidence_id = Some(evidence.evidence_id.clone());
        self.evidence
            .entry(request.run_control_ref.clone())
            .or_default()
            .push(evidence.clone());
        Ok(evidence)
    }

    pub(crate) fn validate_evidence_chain(&self, run_ref: &ManagedRunRefV1) -> AppResult<()> {
        let mut prior = None;
        for (sequence, evidence) in self.evidence.get(run_ref).into_iter().flatten().enumerate() {
            if evidence.contract_version != EFFECT_AUTHORITY_VERSION
                || evidence.run_control_ref != *run_ref
                || evidence.sequence != sequence as u64
                || evidence.prior_evidence_digest != prior
                || compute_evidence_id(evidence)? != evidence.evidence_id
                || compute_evidence_digest(evidence)? != evidence.evidence_digest
                || host_authenticator(&self.evidence_key, evidence)? != evidence.host_authenticator
            {
                return invalid("Host effect evidence chain is forged or out of order.");
            }
            prior = Some(evidence.evidence_digest.clone());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn effect_evidence_for_tests(&self) -> Vec<EffectEvidenceV1> {
        self.evidence.values().flatten().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn effect_envelope_for_tests(
        &self,
        envelope_ref: &EffectEnvelopeRefV1,
    ) -> Option<EffectEnvelopeV1> {
        self.envelopes.get(envelope_ref).cloned()
    }

    /// Requires a complete, successful, ordered evidence chain for an active
    /// run. Write-ahead intents without terminal evidence are indeterminate;
    /// denied/unavailable effects cannot support semantic success.
    pub(crate) fn completion_evidence(
        &self,
        run_ref: &ManagedRunRefV1,
        expected_envelope: &EffectEnvelopeRefV1,
        current: &CurrentHostAuthorityV1,
    ) -> AppResult<CompletionEvidenceV1> {
        let run = self
            .runs
            .get(run_ref)
            .ok_or_else(|| AppError::InvalidInput("Managed run is unavailable.".into()))?;
        let envelope = self
            .envelopes
            .get(expected_envelope)
            .ok_or_else(|| AppError::InvalidInput("Effect envelope is unavailable.".into()))?;
        if run.state != ManagedRunStateV1::Active
            || run.envelope_ref != *expected_envelope
            || envelope.run_control_ref != *run_ref
            || current.now >= run.expires_at
            || current.burned
            || current.disconnected
            || current.restarted
            || !current.bridge_active
            || current.session_binding.expires_at <= current.now
            || current.session_binding.binding_ref != envelope.context.session_binding_ref
            || current.session_binding.local_host_ref != envelope.context.host_ref
        {
            return invalid("Managed run is not currently eligible for Core completion.");
        }
        self.validate_evidence_chain(run_ref)?;
        let evidence = self.evidence.get(run_ref).cloned().unwrap_or_default();
        if evidence.is_empty() || evidence.len() as u64 != run.next_request_sequence {
            return invalid("Managed run evidence is incomplete.");
        }
        let intents = self
            .intents
            .values()
            .filter(|intent| intent.run_control_ref == *run_ref)
            .collect::<Vec<_>>();
        if intents.len() != evidence.len()
            || intents
                .iter()
                .any(|intent| intent.indeterminate || intent.terminal_evidence_id.is_none())
            || evidence
                .iter()
                .any(|item| item.decision != EffectDecisionV1::Allowed)
        {
            return invalid("Managed run has failed, unavailable, or indeterminate evidence.");
        }
        let evidence_head = evidence
            .last()
            .expect("non-empty evidence")
            .evidence_digest
            .clone();
        Ok(CompletionEvidenceV1 {
            envelope: envelope.clone(),
            run: run.clone(),
            evidence,
            evidence_head,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResourceGrantSpecV1 {
    pub(crate) host_ref: HostRef,
    pub(crate) kind: ResourceKindV1,
    pub(crate) safe_identity_ref: String,
    pub(crate) selector_prefix: String,
    pub(crate) allowed_verbs: BTreeSet<ResourceVerbV1>,
    pub(crate) budgets: EffectBudgetsV1,
    pub(crate) expires_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceHashMaterial<'a> {
    contract_version: &'a str,
    prior_evidence_digest: &'a Option<String>,
    request_id: &'a EffectRequestIdV1,
    envelope_ref: &'a EffectEnvelopeRefV1,
    run_control_ref: &'a ManagedRunRefV1,
    context_ref: &'a AuthorityContextRefV1,
    sequence: u64,
    decision: EffectDecisionV1,
    observed_preconditions: &'a [EffectPreconditionV1],
    actual_effect_summary: &'a str,
    budget_debit: &'a EffectBudgetsV1,
    facts: &'a EffectFactsV1,
}

fn evidence_hash_material(evidence: &EffectEvidenceV1) -> EvidenceHashMaterial<'_> {
    EvidenceHashMaterial {
        contract_version: &evidence.contract_version,
        prior_evidence_digest: &evidence.prior_evidence_digest,
        request_id: &evidence.request_id,
        envelope_ref: &evidence.envelope_ref,
        run_control_ref: &evidence.run_control_ref,
        context_ref: &evidence.context_ref,
        sequence: evidence.sequence,
        decision: evidence.decision,
        observed_preconditions: &evidence.observed_preconditions,
        actual_effect_summary: &evidence.actual_effect_summary,
        budget_debit: &evidence.budget_debit,
        facts: &evidence.facts,
    }
}

fn compute_evidence_id(evidence: &EffectEvidenceV1) -> AppResult<EffectEvidenceIdV1> {
    Ok(EffectEvidenceIdV1(domain_hash(
        "pastey-effect-evidence-id-v1",
        &evidence_hash_material(evidence),
    )?))
}

fn compute_evidence_digest(evidence: &EffectEvidenceV1) -> AppResult<String> {
    domain_hash(
        "pastey-effect-evidence-digest-v1",
        &evidence_hash_material(evidence),
    )
}

fn host_authenticator(key: &[u8; 32], evidence: &EffectEvidenceV1) -> AppResult<String> {
    let material = serde_json::to_vec(&evidence_hash_material(evidence))?;
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(b"pastey-host-effect-evidence-authenticator-v1\0");
    hasher.update(&material);
    Ok(format!(
        "host-effect-authenticator:v1:{}",
        hasher.finalize().to_hex()
    ))
}

fn domain_hash(domain: &str, value: &impl Serialize) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(&serde_json::to_vec(value)?);
    Ok(format!("{domain}:{}", hasher.finalize().to_hex()))
}

fn validate_id(value: &str, label: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ID_LEN || value.chars().any(char::is_control) {
        return invalid(&format!("Phase 5 {label} is invalid."));
    }
    Ok(())
}

fn validate_selector_prefix(value: &str) -> AppResult<()> {
    if value.is_empty() {
        return invalid("Resource selector prefix is empty.");
    }
    validate_relative_selector(value)
}

fn validate_relative_selector(value: &str) -> AppResult<()> {
    if value == "." {
        return Ok(());
    }
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return invalid("Resource selector must be normalized and handle-relative.");
    }
    Ok(())
}

fn selector_within_prefix(selector: &str, prefix: &str) -> bool {
    prefix == "." || selector == prefix || selector.starts_with(&format!("{prefix}/"))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_runtime::{DeveloperHostRef, DeveloperTerminalBinding};

    #[derive(Default)]
    struct InMemoryHostEffectBackendV1 {
        resources: HashMap<ResourceHandleRefV1, (u64, String)>,
        processes: HashSet<String>,
        network_actions: usize,
        calls: usize,
        lose_after_intent: bool,
    }

    impl HostEffectBackendV1 for InMemoryHostEffectBackendV1 {
        fn apply(&mut self, request: &EffectRequestV1) -> BackendApplyV1 {
            self.calls += 1;
            if self.lose_after_intent {
                return BackendApplyV1::LostAfterIntent;
            }
            let outcome = match &request.effect {
                EffectRequestKindV1::Resource(effect) => {
                    let entry = self
                        .resources
                        .entry(effect.handle_ref.clone())
                        .or_insert((0, "synthetic-resource-empty".into()));
                    if matches!(
                        effect.verb,
                        ResourceVerbV1::Create
                            | ResourceVerbV1::Replace
                            | ResourceVerbV1::Delete
                            | ResourceVerbV1::SetMetadata
                    ) {
                        entry.0 += 1;
                        entry.1 = effect
                            .value_digest
                            .clone()
                            .unwrap_or_else(|| "synthetic-resource-value".into());
                    }
                    BackendEffectOutcomeV1 {
                        decision: EffectDecisionV1::Allowed,
                        actual_effect_summary: format!("resource_{:?}", effect.verb).to_lowercase(),
                        facts: EffectFactsV1::Resource {
                            handle_ref: effect.handle_ref.clone(),
                            generation: entry.0,
                            content_digest: entry.1.clone(),
                            bytes: request.requested_budget_slice.read_bytes
                                + request.requested_budget_slice.write_bytes,
                        },
                    }
                }
                EffectRequestKindV1::Process(ProcessEffectV1::Spawn { world_ref, .. }) => {
                    let process_ref = format!("synthetic-process:v1:{}", self.processes.len() + 1);
                    self.processes.insert(process_ref.clone());
                    BackendEffectOutcomeV1 {
                        decision: EffectDecisionV1::Allowed,
                        actual_effect_summary: "process_spawn".into(),
                        facts: EffectFactsV1::Process {
                            world_ref: world_ref.clone(),
                            process_ref,
                            state: "synthetic_running".into(),
                        },
                    }
                }
                EffectRequestKindV1::Process(ProcessEffectV1::Signal {
                    world_ref,
                    process_ref,
                    ..
                }) => BackendEffectOutcomeV1 {
                    decision: if self.processes.remove(process_ref) {
                        EffectDecisionV1::Allowed
                    } else {
                        EffectDecisionV1::Denied
                    },
                    actual_effect_summary: "process_signal".into(),
                    facts: EffectFactsV1::Process {
                        world_ref: world_ref.clone(),
                        process_ref: process_ref.clone(),
                        state: "synthetic_stopped".into(),
                    },
                },
                EffectRequestKindV1::Network(effect) => {
                    self.network_actions += 1;
                    BackendEffectOutcomeV1 {
                        decision: EffectDecisionV1::Allowed,
                        actual_effect_summary: format!("network_{:?}", effect.verb).to_lowercase(),
                        facts: EffectFactsV1::Network {
                            scope_ref: effect.scope_ref.clone(),
                            connection_ref: format!(
                                "synthetic-network-action:v1:{}",
                                self.network_actions
                            ),
                            state: "synthetic_complete".into(),
                        },
                    }
                }
            };
            BackendApplyV1::Completed(outcome)
        }
    }

    struct Fixture {
        state: EffectAuthorityStateV1,
        envelope: EffectEnvelopeV1,
        context: AuthorityContextV1,
        current: CurrentHostAuthorityV1,
        workspace: ResourceHandleRefV1,
        output: ResourceHandleRefV1,
        executable: ResourceHandleRefV1,
        scope: NetworkScopeRefV1,
        semantic_ceiling: AuthorityCeilingV1,
    }

    fn budgets(requests: u64) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests,
            read_bytes: 8,
            write_bytes: 8,
            process_spawns: 2,
            process_signals: 2,
            cpu_millis: 100,
            memory_byte_millis: 1_000,
            wall_millis: 100,
            network_resolutions: 2,
            network_connections: 2,
            network_binds: 1,
            network_requests: 2,
            network_bytes: 8,
            network_time_millis: 100,
        }
    }

    fn per_request() -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            read_bytes: 4,
            write_bytes: 4,
            process_spawns: 1,
            process_signals: 1,
            cpu_millis: 50,
            memory_byte_millis: 500,
            wall_millis: 50,
            network_resolutions: 1,
            network_connections: 1,
            network_binds: 1,
            network_requests: 1,
            network_bytes: 4,
            network_time_millis: 50,
        }
    }

    fn fixture(network: bool) -> Fixture {
        let local = HostRef::from_device_id("phase5-local").unwrap();
        let peer = HostRef::from_device_id("phase5-peer").unwrap();
        let binding = HostSessionBinding::new(
            "bridge-phase5",
            local.clone(),
            peer,
            "local-session",
            "peer-session",
            "peer-route",
            1_000,
        )
        .unwrap();
        let context = AuthorityContextV1 {
            contract_version: EFFECT_AUTHORITY_VERSION.into(),
            bridge_id: "bridge-phase5".into(),
            plan_id: "plan-phase5".into(),
            revision_id: "revision-phase5".into(),
            revision_hash: "revision-hash-phase5".into(),
            approval_id: "approval-phase5".into(),
            attempt_id: "attempt-phase5".into(),
            step_id: "transform-phase5".into(),
            semantic_operation: ManagedSemanticOperationV1::Transform,
            participant_ref: PlanParticipantRef::for_host("plan-phase5", &local).unwrap(),
            host_ref: local.clone(),
            admission_ref: "admission-phase5".into(),
            session_binding_ref: binding.binding_ref.clone(),
            input_revisions: vec![ManagedInputRevisionV1 {
                logical_object_id: "object-phase5".into(),
                revision: 1,
                host_ref: local.clone(),
            }],
            issued_at: 100,
            expires_at: 900,
        };
        let mut state = EffectAuthorityStateV1::with_evidence_key([7; 32]);
        let draft = state.begin_run(context.clone()).unwrap();
        let grant =
            |state: &mut EffectAuthorityStateV1, kind, identity: &str, verbs: &[ResourceVerbV1]| {
                state
                    .mint_resource_grant(
                        &draft,
                        ResourceGrantSpecV1 {
                            host_ref: local.clone(),
                            kind,
                            safe_identity_ref: identity.into(),
                            selector_prefix: ".".into(),
                            allowed_verbs: verbs.iter().copied().collect(),
                            budgets: budgets(8),
                            expires_at: 900,
                        },
                    )
                    .unwrap()
            };
        let workspace = grant(
            &mut state,
            ResourceKindV1::Workspace,
            "safe-workspace:v1:one",
            &[ResourceVerbV1::Read, ResourceVerbV1::Replace],
        );
        let output = grant(
            &mut state,
            ResourceKindV1::OutputSlot,
            "safe-output:v1:one",
            &[ResourceVerbV1::Create, ResourceVerbV1::Replace],
        );
        let executable = grant(
            &mut state,
            ResourceKindV1::Executable,
            "safe-executable:v1:one",
            &[ResourceVerbV1::Inspect, ResourceVerbV1::Read],
        );
        let world = ExecutionWorldGrantV1 {
            world_ref: ExecutionWorldRefV1(
                domain_hash(
                    "pastey-execution-world-v1",
                    &(draft.context_ref.as_str(), draft.run_control_ref.as_str()),
                )
                .unwrap(),
            ),
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            world_identity_digest: "sealed-world-digest:v1:one".into(),
            mounted_resources: [
                workspace.handle_ref.clone(),
                output.handle_ref.clone(),
                executable.handle_ref.clone(),
            ]
            .into_iter()
            .collect(),
            executable_resources: [executable.handle_ref.clone()].into_iter().collect(),
            required_properties: [
                ConfinementPropertyV1::AuthorizedResourceProjection,
                ConfinementPropertyV1::AuthorityNeutralEnvironment,
                ConfinementPropertyV1::ExplicitProcessIo,
                ConfinementPropertyV1::PlatformSandboxedProcess,
                ConfinementPropertyV1::CancellableProcessSession,
                ConfinementPropertyV1::NoRawNetwork,
            ]
            .into_iter()
            .collect(),
            budgets: budgets(8),
            expires_at: 900,
        };
        let scope = NetworkScopeRefV1(
            domain_hash("pastey-network-scope-v1", &"approved-api-scope").unwrap(),
        );
        let network_grant = NetworkGrantV1 {
            context_ref: draft.context_ref.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            host_ref: local.clone(),
            scope_refs: [scope.clone()].into_iter().collect(),
            allowed_verbs: [NetworkVerbV1::Resolve, NetworkVerbV1::Connect]
                .into_iter()
                .collect(),
            destination_refs: ["network-destination:v1:approved".into()]
                .into_iter()
                .collect(),
            budgets: budgets(8),
            expires_at: 900,
        };
        let ceiling = AuthorityCeilingV1 {
            context_ref: draft.context_ref.clone(),
            source_snapshot_ref: "authority-source:v1:semantic".into(),
            resources: vec![workspace.clone(), output.clone(), executable.clone()],
            world: world.clone(),
            effect_bounds: vec![
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Resource(ResourceVerbV1::Read),
                    max_per_request: per_request(),
                },
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Resource(ResourceVerbV1::Replace),
                    max_per_request: per_request(),
                },
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Process(ProcessVerbV1::Spawn),
                    max_per_request: per_request(),
                },
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Process(ProcessVerbV1::Signal),
                    max_per_request: per_request(),
                },
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Network(NetworkVerbV1::Resolve),
                    max_per_request: per_request(),
                },
                EffectBoundV1 {
                    capability: EffectCapabilityV1::Network(NetworkVerbV1::Connect),
                    max_per_request: per_request(),
                },
            ],
            budgets: budgets(8),
            network: if network {
                NetworkAuthorityV1::Scoped(Box::new(network_grant))
            } else {
                NetworkAuthorityV1::Denied
            },
            expires_at: 900,
        };
        let envelope = compile_effect_envelope(EffectEnvelopeCompileRequestV1 {
            context: context.clone(),
            run_control_ref: draft.run_control_ref.clone(),
            semantic_ceiling: ceiling.clone(),
            admission_ceiling: ceiling.clone(),
            host_policy_ceiling: ceiling.clone(),
            confinement_ceiling: ceiling.clone(),
            host_policy_snapshot_ref: "host-policy:v1:test".into(),
            result_contract: ResultContractV1::Transform {
                input: context.input_revisions[0].clone(),
                output_revision: 2,
                output_slot: output.handle_ref.clone(),
            },
        })
        .unwrap();
        state.install_envelope(draft, envelope.clone()).unwrap();
        state.activate_run(&envelope.run_control_ref, 101).unwrap();
        Fixture {
            state,
            envelope,
            context,
            current: CurrentHostAuthorityV1 {
                session_binding: binding,
                bridge_active: true,
                burned: false,
                disconnected: false,
                restarted: false,
                now: 110,
            },
            workspace: workspace.handle_ref,
            output: output.handle_ref,
            executable: executable.handle_ref,
            scope,
            semantic_ceiling: ceiling,
        }
    }

    fn read_effect(handle: ResourceHandleRefV1) -> EffectRequestKindV1 {
        EffectRequestKindV1::Resource(ResourceEffectV1 {
            verb: ResourceVerbV1::Read,
            handle_ref: handle,
            relative_selector: "src/main.rs".into(),
            value_digest: None,
        })
    }

    #[test]
    fn resource_projection_intersects_grant_verbs_with_envelope_bounds() {
        let fixture = fixture(false);
        let (grant, effective) = fixture
            .state
            .validate_resource_projection_attachment(
                &fixture.output,
                ResourceKindV1::OutputSlot,
                &fixture.envelope.envelope_ref,
                &fixture.envelope.run_control_ref,
                &fixture.context,
                &fixture.current,
            )
            .unwrap();
        assert!(grant.allowed_verbs.contains(&ResourceVerbV1::Create));
        assert!(grant.allowed_verbs.contains(&ResourceVerbV1::Replace));
        assert!(!effective.contains(&ResourceVerbV1::Create));
        assert!(effective.contains(&ResourceVerbV1::Replace));
    }

    fn read_budget(bytes: u64) -> EffectBudgetsV1 {
        EffectBudgetsV1 {
            requests: 1,
            read_bytes: bytes,
            ..EffectBudgetsV1::default()
        }
    }

    fn lower_one(
        fixture: &Fixture,
        name: &str,
        sequence: u64,
        effect: EffectRequestKindV1,
        budget: EffectBudgetsV1,
    ) -> EffectRequestV1 {
        lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: fixture.context.clone(),
                envelope_ref: fixture.envelope.envelope_ref.clone(),
                run_control_ref: fixture.envelope.run_control_ref.clone(),
                first_sequence: sequence,
            },
            &ToolRequestV1 {
                tool_name: name.into(),
                adapter_version_ref: "synthetic-adapter:v1".into(),
                intents: vec![ToolEffectIntentV1 {
                    effect,
                    requested_budget_slice: budget,
                    preconditions: Vec::new(),
                }],
            },
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn identities_are_canonical_and_domain_separated() {
        let fixture = fixture(false);
        assert_eq!(
            fixture.context.context_ref().unwrap(),
            fixture.envelope.context_ref
        );
        assert!(fixture
            .envelope
            .context_ref
            .as_str()
            .starts_with("pastey-authority-context-v1:"));
        assert!(fixture
            .envelope
            .envelope_ref
            .as_str()
            .starts_with("pastey-effect-envelope-v1:"));
        assert!(fixture
            .envelope
            .run_control_ref
            .as_str()
            .starts_with("pastey-managed-run-v1:"));
        assert_ne!(
            fixture.envelope.context_ref.as_str(),
            fixture.envelope.envelope_ref.as_str()
        );
        assert_eq!(
            compute_envelope_ref(&fixture.envelope).unwrap(),
            fixture.envelope.envelope_ref
        );
    }

    #[test]
    fn envelope_is_only_the_intersection_and_widening_is_rejected() {
        let fixture = fixture(true);
        let mut policy = fixture.semantic_ceiling.clone();
        policy
            .effect_bounds
            .retain(|bound| !matches!(bound.capability, EffectCapabilityV1::Process(_)));
        policy.network = NetworkAuthorityV1::Denied;
        policy.budgets.requests = 2;
        let draft = ManagedRunDraftV1 {
            context: fixture.context.clone(),
            context_ref: fixture.envelope.context_ref.clone(),
            run_control_ref: fixture.envelope.run_control_ref.clone(),
        };
        let envelope = compile_effect_envelope(EffectEnvelopeCompileRequestV1 {
            context: draft.context,
            run_control_ref: draft.run_control_ref,
            semantic_ceiling: fixture.semantic_ceiling.clone(),
            admission_ceiling: fixture.semantic_ceiling.clone(),
            host_policy_ceiling: policy.clone(),
            confinement_ceiling: fixture.semantic_ceiling.clone(),
            host_policy_snapshot_ref: "host-policy:v1:smaller".into(),
            result_contract: fixture.envelope.result_contract.clone(),
        })
        .unwrap();
        assert_eq!(envelope.budgets.requests, 2);
        assert_eq!(envelope.network, NetworkAuthorityV1::Denied);
        assert!(!envelope
            .effect_bounds
            .iter()
            .any(|bound| matches!(bound.capability, EffectCapabilityV1::Process(_))));

        let mut widened = envelope;
        widened.effect_bounds.push(EffectBoundV1 {
            capability: EffectCapabilityV1::Process(ProcessVerbV1::Spawn),
            max_per_request: per_request(),
        });
        assert!(validate_envelope_subset(&widened, &policy).is_err());
        // Keep the original fixture live only to prove compilation did not
        // mutate the installed authority state.
        assert_eq!(
            fixture
                .state
                .runs
                .get(&fixture.envelope.run_control_ref)
                .unwrap()
                .state,
            ManagedRunStateV1::Active
        );
    }

    #[test]
    fn exact_context_and_host_session_substitutions_fail_closed() {
        let mut fixture = fixture(false);
        let base = lower_one(
            &fixture,
            "synthetic-document-tool",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        let mut substitutions = Vec::new();
        let mut wrong_step = base.clone();
        wrong_step.context.step_id = "other-step".into();
        substitutions.push(wrong_step);
        let mut wrong_revision = base.clone();
        wrong_revision.context.revision_hash = "other-revision-hash".into();
        substitutions.push(wrong_revision);
        let mut wrong_attempt = base.clone();
        wrong_attempt.context.attempt_id = "other-attempt".into();
        substitutions.push(wrong_attempt);
        let mut wrong_admission = base.clone();
        wrong_admission.context.admission_ref = "other-admission".into();
        substitutions.push(wrong_admission);
        let other_host = HostRef::from_device_id("substitute-host").unwrap();
        let mut wrong_host = base.clone();
        wrong_host.context.host_ref = other_host.clone();
        wrong_host.context.participant_ref =
            PlanParticipantRef::for_host(&wrong_host.context.plan_id, &other_host).unwrap();
        for input in &mut wrong_host.context.input_revisions {
            input.host_ref = other_host.clone();
        }
        substitutions.push(wrong_host);
        for request in &mut substitutions {
            request.request_id = compute_request_id(request).unwrap();
            assert!(fixture
                .state
                .enforce(
                    request,
                    &fixture.current,
                    &mut InMemoryHostEffectBackendV1::default()
                )
                .is_err());
        }

        let mut wrong_session = fixture.current.clone();
        wrong_session.session_binding = HostSessionBinding::new(
            "bridge-phase5",
            fixture.context.host_ref.clone(),
            HostRef::from_device_id("phase5-peer").unwrap(),
            "local-session-new",
            "peer-session-new",
            "peer-route-new",
            1_000,
        )
        .unwrap();
        assert!(fixture
            .state
            .enforce(
                &base,
                &wrong_session,
                &mut InMemoryHostEffectBackendV1::default()
            )
            .is_err());
        assert_eq!(
            fixture
                .state
                .runs
                .get(&fixture.envelope.run_control_ref)
                .unwrap()
                .next_request_sequence,
            0
        );
    }

    #[test]
    fn run_lifecycle_is_core_owned_and_terminal_states_never_reopen() {
        let mut fixture = fixture(false);
        fixture
            .state
            .cancel_run(&fixture.envelope.run_control_ref)
            .unwrap();
        let request = lower_one(
            &fixture,
            "synthetic-source-editor",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        assert!(fixture
            .state
            .enforce(
                &request,
                &fixture.current,
                &mut InMemoryHostEffectBackendV1::default()
            )
            .is_err());
        fixture
            .state
            .finish_run(&fixture.envelope.run_control_ref)
            .unwrap();
        assert!(fixture
            .state
            .activate_run(&fixture.envelope.run_control_ref, 120)
            .is_err());
        assert!(fixture
            .state
            .revoke_run(&fixture.envelope.run_control_ref)
            .is_err());
    }

    #[test]
    fn replay_and_skipped_sequences_are_rejected_without_consuming_authority() {
        let mut fixture = fixture(false);
        let out_of_order = lower_one(
            &fixture,
            "synthetic-test-runner",
            1,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        let mut backend = InMemoryHostEffectBackendV1::default();
        assert!(fixture
            .state
            .enforce(&out_of_order, &fixture.current, &mut backend)
            .is_err());
        let first = lower_one(
            &fixture,
            "synthetic-test-runner",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        assert_eq!(
            fixture
                .state
                .enforce(&first, &fixture.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Allowed
        );
        assert!(fixture
            .state
            .enforce(&first, &fixture.current, &mut backend)
            .is_err());
        assert_eq!(backend.calls, 1);
    }

    #[test]
    fn cumulative_budget_cannot_reset_on_retry_or_split_requests() {
        let mut fixture = fixture(false);
        fixture
            .state
            .envelopes
            .get_mut(&fixture.envelope.envelope_ref)
            .unwrap()
            .budgets
            .read_bytes = 4;
        let mut backend = InMemoryHostEffectBackendV1::default();
        for sequence in 0..2 {
            let request = lower_one(
                &fixture,
                "synthetic-document-tool",
                sequence,
                read_effect(fixture.workspace.clone()),
                read_budget(2),
            );
            assert_eq!(
                fixture
                    .state
                    .enforce(&request, &fixture.current, &mut backend)
                    .unwrap()
                    .decision,
                EffectDecisionV1::Allowed
            );
            if sequence == 0 {
                assert!(fixture
                    .state
                    .enforce(&request, &fixture.current, &mut backend)
                    .is_err());
            }
        }
        let third = lower_one(
            &fixture,
            "synthetic-source-editor",
            2,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        let evidence = fixture
            .state
            .enforce(&third, &fixture.current, &mut backend)
            .unwrap();
        assert_eq!(evidence.decision, EffectDecisionV1::Denied);
        assert_eq!(backend.calls, 2);
        assert_eq!(
            fixture
                .state
                .runs
                .get(&fixture.envelope.run_control_ref)
                .unwrap()
                .cumulative_budget_debits
                .read_bytes,
            4
        );
    }

    #[test]
    fn network_is_independent_default_deny_and_never_inferred_from_tool_name() {
        let mut denied = fixture(false);
        let network_effect = EffectRequestKindV1::Network(NetworkEffectV1 {
            verb: NetworkVerbV1::Connect,
            scope_ref: denied.scope.clone(),
            destination_ref: "network-destination:v1:approved".into(),
            transport_ref: "transport:v1:synthetic-tls".into(),
            resolution_generation_ref: None,
            request_digest: None,
        });
        let cargo_named = lower_one(
            &denied,
            "cargo-with-package-install-claim",
            0,
            network_effect.clone(),
            EffectBudgetsV1 {
                requests: 1,
                network_connections: 1,
                network_bytes: 1,
                ..EffectBudgetsV1::default()
            },
        );
        let http_named = lower_one(
            &denied,
            "synthetic-http-tool",
            0,
            network_effect.clone(),
            cargo_named.requested_budget_slice,
        );
        assert_eq!(cargo_named.request_id, http_named.request_id);
        let mut backend = InMemoryHostEffectBackendV1::default();
        assert_eq!(
            denied
                .state
                .enforce(&cargo_named, &denied.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Denied
        );
        assert_eq!(backend.calls, 0);

        let mut allowed = fixture(true);
        let request = lower_one(
            &allowed,
            "unrelated-synthetic-tool-name",
            0,
            network_effect,
            cargo_named.requested_budget_slice,
        );
        assert_eq!(
            allowed
                .state
                .enforce(&request, &allowed.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Allowed
        );
        assert_eq!(backend.network_actions, 1);
    }

    #[test]
    fn resource_process_and_network_authority_are_orthogonal() {
        let mut fixture = fixture(false);
        let composite = ToolRequestV1 {
            tool_name: "synthetic-mixed-harness-tool".into(),
            adapter_version_ref: "synthetic-adapter:v1".into(),
            intents: vec![
                ToolEffectIntentV1 {
                    effect: read_effect(fixture.workspace.clone()),
                    requested_budget_slice: read_budget(1),
                    preconditions: Vec::new(),
                },
                ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Process(ProcessEffectV1::Spawn {
                        world_ref: fixture.envelope.world.world_ref.clone(),
                        executable_handle: fixture.executable.clone(),
                        argv_digest: "argv-digest:v1:synthetic".into(),
                        working_directory_handle: Some(fixture.workspace.clone()),
                        working_directory_selector: Some(".".into()),
                        environment_digest: "environment-digest:v1:empty".into(),
                        stdin_digest: None,
                    }),
                    requested_budget_slice: EffectBudgetsV1 {
                        requests: 1,
                        process_spawns: 1,
                        cpu_millis: 1,
                        wall_millis: 1,
                        ..EffectBudgetsV1::default()
                    },
                    preconditions: Vec::new(),
                },
                ToolEffectIntentV1 {
                    effect: EffectRequestKindV1::Network(NetworkEffectV1 {
                        verb: NetworkVerbV1::Resolve,
                        scope_ref: fixture.scope.clone(),
                        destination_ref: "network-destination:v1:approved".into(),
                        transport_ref: "transport:v1:synthetic".into(),
                        resolution_generation_ref: None,
                        request_digest: None,
                    }),
                    requested_budget_slice: EffectBudgetsV1 {
                        requests: 1,
                        network_resolutions: 1,
                        ..EffectBudgetsV1::default()
                    },
                    preconditions: Vec::new(),
                },
            ],
        };
        let requests = lower_tool_request(
            &StepWorkDescriptorV1 {
                contract_version: EFFECT_AUTHORITY_VERSION.into(),
                context: fixture.context.clone(),
                envelope_ref: fixture.envelope.envelope_ref.clone(),
                run_control_ref: fixture.envelope.run_control_ref.clone(),
                first_sequence: 0,
            },
            &composite,
        )
        .unwrap();
        let mut backend = InMemoryHostEffectBackendV1::default();
        assert_eq!(
            fixture
                .state
                .enforce(&requests[0], &fixture.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Allowed
        );
        assert_eq!(
            fixture
                .state
                .enforce(&requests[1], &fixture.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Allowed
        );
        assert_eq!(
            fixture
                .state
                .enforce(&requests[2], &fixture.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Denied
        );
    }

    #[test]
    fn lowering_is_pure_deterministic_and_tool_names_confer_no_authority() {
        let fixture = fixture(false);
        let effect = read_effect(fixture.workspace.clone());
        let first = lower_one(
            &fixture,
            "synthetic-source-editor",
            0,
            effect.clone(),
            read_budget(1),
        );
        let second = lower_one(
            &fixture,
            "synthetic-document-editor",
            0,
            effect,
            read_budget(1),
        );
        assert_eq!(first, second);
        assert_eq!(
            first.request_id,
            compute_request_id(&first).expect("canonical request identity")
        );
    }

    #[test]
    fn unknown_contract_versions_and_effect_families_fail_closed() {
        assert!(
            serde_json::from_value::<EffectRequestKindV1>(serde_json::json!({
                "family": "device",
                "effect": {"verb": "open"}
            }))
            .is_err()
        );
        let mut fixture = fixture(false);
        let mut request = lower_one(
            &fixture,
            "synthetic-tool",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        request.contract_version = "pastey-effect-authority-v2".into();
        assert!(fixture
            .state
            .enforce(
                &request,
                &fixture.current,
                &mut InMemoryHostEffectBackendV1::default()
            )
            .is_err());
    }

    #[test]
    fn a_handle_owned_by_another_envelope_or_run_is_rejected() {
        let mut fixture = fixture(false);
        let record = fixture.state.resources.get_mut(&fixture.workspace).unwrap();
        record.envelope_ref = EffectEnvelopeRefV1("pastey-effect-envelope-v1:foreign".into());
        record.grant.run_control_ref = ManagedRunRefV1("pastey-managed-run-v1:foreign".into());
        let request = lower_one(
            &fixture,
            "synthetic-source-editor",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        let mut backend = InMemoryHostEffectBackendV1::default();
        assert_eq!(
            fixture
                .state
                .enforce(&request, &fixture.current, &mut backend)
                .unwrap()
                .decision,
            EffectDecisionV1::Denied
        );
        assert_eq!(backend.calls, 0);
    }

    #[test]
    fn burn_restart_disconnect_and_expiry_invalidate_effect_authority() {
        for invalidation in ["burn", "restart", "disconnect", "expiry"] {
            let mut fixture = fixture(false);
            let request = lower_one(
                &fixture,
                "synthetic-tool",
                0,
                read_effect(fixture.workspace.clone()),
                read_budget(1),
            );
            match invalidation {
                "burn" => fixture.state.revoke_bridge(&fixture.context.bridge_id),
                "restart" => fixture.current.restarted = true,
                "disconnect" => fixture.current.disconnected = true,
                "expiry" => fixture.current.now = fixture.context.expires_at,
                _ => unreachable!(),
            }
            assert!(fixture
                .state
                .enforce(
                    &request,
                    &fixture.current,
                    &mut InMemoryHostEffectBackendV1::default()
                )
                .is_err());
        }
        // A newly constructed process-local store carries no pre-restart run.
        assert!(EffectAuthorityStateV1::with_evidence_key([7; 32])
            .runs
            .is_empty());
    }

    #[test]
    fn developer_terminal_identity_and_binding_cannot_enter_managed_authority() {
        let terminal_binding =
            DeveloperTerminalBinding::new("bridge", "controller", "target", "route");
        assert!(HostRef::parse(terminal_binding.controller_host.0.clone()).is_err());
        assert!(HostRef::parse(terminal_binding.target_host.0.clone()).is_err());
        assert!(serde_json::from_value::<AuthorityContextV1>(
            serde_json::to_value(&terminal_binding).unwrap()
        )
        .is_err());
        let developer_host = DeveloperHostRef("developer-host:v0:terminal".into());
        assert!(HostRef::parse(developer_host.0).is_err());
    }

    #[test]
    fn evidence_is_host_authenticated_ordered_and_worker_tampering_fails() {
        let mut fixture = fixture(false);
        let mut backend = InMemoryHostEffectBackendV1::default();
        for sequence in 0..2 {
            let request = lower_one(
                &fixture,
                "synthetic-document-tool",
                sequence,
                read_effect(fixture.workspace.clone()),
                read_budget(1),
            );
            fixture
                .state
                .enforce(&request, &fixture.current, &mut backend)
                .unwrap();
        }
        fixture
            .state
            .validate_evidence_chain(&fixture.envelope.run_control_ref)
            .unwrap();
        let chain = fixture
            .state
            .evidence
            .get(&fixture.envelope.run_control_ref)
            .unwrap();
        assert_eq!(
            chain[1].prior_evidence_digest,
            Some(chain[0].evidence_digest.clone())
        );
        fixture
            .state
            .evidence
            .get_mut(&fixture.envelope.run_control_ref)
            .unwrap()[0]
            .actual_effect_summary = "worker-forged-success".into();
        assert!(fixture
            .state
            .validate_evidence_chain(&fixture.envelope.run_control_ref)
            .is_err());
    }

    #[test]
    fn write_ahead_without_terminal_evidence_is_indeterminate_and_revoked() {
        let mut fixture = fixture(false);
        let request = lower_one(
            &fixture,
            "synthetic-source-editor",
            0,
            read_effect(fixture.workspace.clone()),
            read_budget(1),
        );
        let mut backend = InMemoryHostEffectBackendV1 {
            lose_after_intent: true,
            ..InMemoryHostEffectBackendV1::default()
        };
        assert!(fixture
            .state
            .enforce(&request, &fixture.current, &mut backend)
            .is_err());
        let intent = fixture.state.intents.get(&request.request_id).unwrap();
        assert!(intent.indeterminate);
        assert!(intent.terminal_evidence_id.is_none());
        assert!(!fixture
            .state
            .evidence
            .contains_key(&fixture.envelope.run_control_ref));
        assert_eq!(
            fixture
                .state
                .runs
                .get(&fixture.envelope.run_control_ref)
                .unwrap()
                .state,
            ManagedRunStateV1::Interrupted
        );
        assert!(fixture
            .state
            .enforce(&request, &fixture.current, &mut backend)
            .is_err());
    }

    #[test]
    fn fake_and_unavailable_backends_cannot_create_lineage_or_execute_results() {
        let mut first_fixture = fixture(false);
        let request = lower_one(
            &first_fixture,
            "synthetic-source-editor",
            0,
            EffectRequestKindV1::Resource(ResourceEffectV1 {
                verb: ResourceVerbV1::Replace,
                handle_ref: first_fixture.output.clone(),
                relative_selector: "result.bin".into(),
                value_digest: Some("synthetic-output-digest".into()),
            }),
            EffectBudgetsV1 {
                requests: 1,
                write_bytes: 1,
                ..EffectBudgetsV1::default()
            },
        );
        let evidence = first_fixture
            .state
            .enforce(
                &request,
                &first_fixture.current,
                &mut InMemoryHostEffectBackendV1::default(),
            )
            .unwrap();
        let facts_json = serde_json::to_string(&evidence.facts).unwrap();
        assert!(!facts_json.contains("logicalObjectId"));
        assert!(!facts_json.contains("outputRevision"));

        let mut unavailable_fixture = fixture(false);
        let request = lower_one(
            &unavailable_fixture,
            "synthetic-document-tool",
            0,
            read_effect(unavailable_fixture.workspace.clone()),
            read_budget(1),
        );
        assert_eq!(
            unavailable_fixture
                .state
                .enforce(
                    &request,
                    &unavailable_fixture.current,
                    &mut UnavailableProductionEffectBackendV1,
                )
                .unwrap()
                .decision,
            EffectDecisionV1::Unavailable
        );
    }
}

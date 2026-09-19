//! Proposal-only Natural-v2 lowering into the deterministic native-v2 Composer.
//!
//! Candidate aliases are model-visible labels, never authority. This module
//! resolves them against the requester's current Host-owned Bridge/object
//! selections, derives logical Search outputs itself, and creates only a Draft
//! review candidate. Approval and attempt start remain separate Core paths.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    bridge_plan::StepOperation,
    error::{AppError, AppResult},
    host_identity::HostRef,
    host_runtime::HostRuntime,
    models::BridgePeerLiveness,
    native_v2_orchestration::{
        NativeV2ComposeRequestV1, NativeV2ObjectRevisionDtoV1, NativeV2PlanStatusV1,
        NativeV2RootDraftV1, NativeV2StepDraftV1,
    },
    storage,
};

pub(crate) const NATURAL_V2_SCHEMA_VERSION: &str = "candidate-semantic-plan-v2";
pub(crate) const NATURAL_V2_REVIEW_SCHEMA_VERSION: &str = "pastey-natural-v2-review-v1";
const MAX_ITEMS: usize = 64;
const MAX_ID: usize = 128;
const MAX_TEXT: usize = 1_024;
const MAX_FACT: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2HostSelectionV1 {
    pub alias: String,
    pub host_ref: String,
    pub display_name: String,
    #[serde(default)]
    pub capability_facts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2RootSelectionV1 {
    pub root_alias: String,
    pub object_alias: String,
    pub logical_object_id: String,
    pub revision: u64,
    pub host_alias: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2TransferSelectionV1 {
    pub source_host_alias: String,
    pub destination_host_alias: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2ProposalContextV1 {
    pub hosts: Vec<NaturalV2HostSelectionV1>,
    #[serde(default)]
    pub roots: Vec<NaturalV2RootSelectionV1>,
    pub allowed_operations: Vec<StepOperation>,
    #[serde(default)]
    pub allowed_transfer_routes: Vec<NaturalV2TransferSelectionV1>,
    #[serde(default)]
    pub allowed_scope_labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateSemanticRootV2 {
    pub root_alias: String,
    pub object_alias: String,
    pub host_alias: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CandidateSemanticStepV2 {
    Search {
        step_alias: String,
        depends_on: Vec<String>,
        host_alias: String,
        output_alias: String,
        query: String,
        safe_scope_labels: Vec<String>,
    },
    Transform {
        step_alias: String,
        depends_on: Vec<String>,
        host_alias: String,
        input_alias: String,
        output_alias: String,
        modification_intent: String,
    },
    Transfer {
        step_alias: String,
        depends_on: Vec<String>,
        source_host_alias: String,
        destination_host_alias: String,
        input_alias: String,
        output_alias: String,
    },
    Execute {
        step_alias: String,
        depends_on: Vec<String>,
        host_alias: String,
        target_alias: String,
        execution_intent: String,
    },
}

impl CandidateSemanticStepV2 {
    fn alias(&self) -> &str {
        match self {
            Self::Search { step_alias, .. }
            | Self::Transform { step_alias, .. }
            | Self::Transfer { step_alias, .. }
            | Self::Execute { step_alias, .. } => step_alias,
        }
    }

    fn dependencies(&self) -> &[String] {
        match self {
            Self::Search { depends_on, .. }
            | Self::Transform { depends_on, .. }
            | Self::Transfer { depends_on, .. }
            | Self::Execute { depends_on, .. } => depends_on,
        }
    }

    fn operation(&self) -> StepOperation {
        match self {
            Self::Search { .. } => StepOperation::Search,
            Self::Transform { .. } => StepOperation::Transform,
            Self::Transfer { .. } => StepOperation::Transfer,
            Self::Execute { .. } => StepOperation::Execute,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateSemanticPlanV2 {
    pub schema_version: String,
    pub title: String,
    pub original_user_goal: String,
    pub expected_outcome: String,
    #[serde(default)]
    pub roots: Vec<CandidateSemanticRootV2>,
    pub steps: Vec<CandidateSemanticStepV2>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2ComposeCandidateRequestV1 {
    pub plan_id: String,
    pub revision_id: String,
    pub revision_number: u32,
    pub bridge_id: String,
    pub requester_host_alias: String,
    /// Host-captured user input. The candidate must repeat it exactly; provider
    /// output cannot silently replace the requested goal.
    pub original_user_goal: String,
    pub context: NaturalV2ProposalContextV1,
    pub candidate: CandidateSemanticPlanV2,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2AffectedHostV1 {
    pub host_alias: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2TopologyStepV1 {
    pub step_alias: String,
    pub operation: StepOperation,
    pub host_aliases: Vec<String>,
    pub depends_on: Vec<String>,
    pub input_alias: Option<String>,
    pub output_alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2MovementV1 {
    pub step_alias: String,
    pub object_alias: String,
    pub source_host_alias: String,
    pub destination_host_alias: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NaturalV2CandidateReviewV1 {
    pub schema_version: String,
    pub title: String,
    pub draft: NativeV2PlanStatusV1,
    pub affected_hosts: Vec<NaturalV2AffectedHostV1>,
    pub topology: Vec<NaturalV2TopologyStepV1>,
    pub movements: Vec<NaturalV2MovementV1>,
}

#[derive(Clone)]
struct ObjectState {
    object: NativeV2ObjectRevisionDtoV1,
    host_alias: String,
    producer: Option<String>,
}

impl HostRuntime {
    /// Revalidates aliases against current Host state and creates a Draft only.
    pub(crate) fn compose_natural_v2_candidate(
        &self,
        request: NaturalV2ComposeCandidateRequestV1,
        now: i64,
    ) -> AppResult<NaturalV2CandidateReviewV1> {
        self.revalidate_natural_v2_context(&request, now)?;
        let (compose, review) = lower_candidate(request)?;
        let draft = self.compose_native_v2_product_plan(compose, now)?;
        Ok(NaturalV2CandidateReviewV1 { draft, ..review })
    }

    fn revalidate_natural_v2_context(
        &self,
        request: &NaturalV2ComposeCandidateRequestV1,
        now: i64,
    ) -> AppResult<()> {
        storage::get_room_by_id(&self.paths, &request.bridge_id)?;
        let peers = storage::list_bridge_peer_endpoints(&self.paths, &request.bridge_id)?;
        let requester = request
            .context
            .hosts
            .iter()
            .find(|host| host.alias == request.requester_host_alias)
            .ok_or_else(|| invalid_error("Natural-v2 requester Host alias is unavailable."))?;
        if requester.host_ref != self.local_host_ref.as_str() {
            return invalid("Only the local requester Host may create a Natural-v2 review.");
        }
        let mut selected_hosts = HashSet::new();
        for host in &request.context.hosts {
            let host_ref = HostRef::parse(host.host_ref.clone())?;
            if !selected_hosts.insert(host_ref.clone()) {
                return invalid("Natural-v2 context contains a duplicate Host selection.");
            }
            if host_ref == self.local_host_ref {
                continue;
            }
            let matches = peers
                .iter()
                .filter(|peer| {
                    peer.logical_host_ref.as_deref() == Some(host_ref.as_str())
                        && !matches!(
                            peer.liveness,
                            BridgePeerLiveness::Left
                                | BridgePeerLiveness::Stale
                                | BridgePeerLiveness::Expired
                        )
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return invalid("Natural-v2 Host alias is stale, unavailable, or ambiguous.");
            }
            if matches[0]
                .display_name
                .as_deref()
                .is_some_and(|display_name| display_name.trim() != host.display_name.trim())
            {
                return invalid("Natural-v2 Host display selection is stale or substituted.");
            }
        }
        for root in &request.context.roots {
            let host = request
                .context
                .hosts
                .iter()
                .find(|host| host.alias == root.host_alias)
                .ok_or_else(|| invalid_error("Natural-v2 root names an unknown Host alias."))?;
            if host.host_ref != self.local_host_ref.as_str() {
                return invalid("Natural-v2 root selection is not bound at the requesting Host.");
            }
            let acquisition = self.managed_objects.lock().acquisition_for_revision(
                &request.bridge_id,
                &root.logical_object_id,
                root.revision,
                now,
            )?;
            if acquisition.object.host_ref.as_str() != host.host_ref {
                return invalid("Natural-v2 root Host/object selection is stale or substituted.");
            }
        }
        Ok(())
    }
}

pub(crate) fn lower_candidate(
    request: NaturalV2ComposeCandidateRequestV1,
) -> AppResult<(NativeV2ComposeRequestV1, NaturalV2CandidateReviewV1)> {
    validate_request_shape(&request)?;
    if request.candidate.original_user_goal != request.original_user_goal {
        return invalid("Natural-v2 candidate changed the Host-captured user goal.");
    }
    let hosts = request
        .context
        .hosts
        .iter()
        .map(|host| (host.alias.clone(), host.clone()))
        .collect::<BTreeMap<_, _>>();
    let requester = hosts
        .get(&request.requester_host_alias)
        .ok_or_else(|| invalid_error("Natural-v2 requester Host alias is unavailable."))?;
    let selected_roots = request
        .context
        .roots
        .iter()
        .map(|root| (root.root_alias.clone(), root.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut objects = HashMap::<String, ObjectState>::new();
    let mut native_roots = Vec::new();
    for root in &request.candidate.roots {
        let selected = selected_roots
            .get(&root.root_alias)
            .ok_or_else(|| invalid_error("Natural-v2 candidate names an unprovided root alias."))?;
        if root.object_alias != selected.object_alias || root.host_alias != selected.host_alias {
            return invalid("Natural-v2 candidate substituted a root object or Host alias.");
        }
        if objects
            .insert(
                root.object_alias.clone(),
                ObjectState {
                    object: NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: selected.logical_object_id.clone(),
                        revision: selected.revision,
                    },
                    host_alias: selected.host_alias.clone(),
                    producer: None,
                },
            )
            .is_some()
        {
            return invalid("Natural-v2 candidate contains a duplicate root object alias.");
        }
        native_roots.push(NativeV2RootDraftV1 {
            root_id: root.root_alias.clone(),
            object: NativeV2ObjectRevisionDtoV1 {
                logical_object_id: selected.logical_object_id.clone(),
                revision: selected.revision,
            },
            host_ref: host_ref(&hosts, &root.host_alias)?,
        });
    }

    let allowed_operations = &request.context.allowed_operations;
    let allowed_scopes = request
        .context
        .allowed_scope_labels
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let allowed_transfers = request
        .context
        .allowed_transfer_routes
        .iter()
        .map(|route| {
            (
                route.source_host_alias.clone(),
                route.destination_host_alias.clone(),
            )
        })
        .collect::<HashSet<_>>();
    let mut native_steps = Vec::new();
    let mut topology = Vec::new();
    let mut movements = Vec::new();
    let mut seen_steps = HashSet::<String>::new();
    for step in &request.candidate.steps {
        if !allowed_operations.contains(&step.operation()) {
            return invalid("Natural-v2 candidate requested an operation outside the selected proposal vocabulary.");
        }
        let step_alias = step.alias().to_string();
        let dependencies = step.dependencies().to_vec();
        if !seen_steps.insert(step_alias.clone())
            || dependencies
                .iter()
                .any(|dependency| !seen_steps.contains(dependency))
        {
            return invalid(
                "Natural-v2 step aliases/dependencies are duplicate, forward, or unknown.",
            );
        }
        match step {
            CandidateSemanticStepV2::Search {
                host_alias,
                output_alias,
                query,
                safe_scope_labels,
                ..
            } => {
                if safe_scope_labels.is_empty()
                    || safe_scope_labels
                        .iter()
                        .any(|scope| !allowed_scopes.contains(scope))
                    || objects.contains_key(output_alias)
                {
                    return invalid(
                        "Natural-v2 Search output or reviewed scope selection is invalid.",
                    );
                }
                let object = NativeV2ObjectRevisionDtoV1 {
                    logical_object_id: derived_search_object_id(
                        &request.plan_id,
                        &request.revision_id,
                        &step_alias,
                    ),
                    revision: 1,
                };
                objects.insert(
                    output_alias.clone(),
                    ObjectState {
                        object: object.clone(),
                        host_alias: host_alias.clone(),
                        producer: Some(step_alias.clone()),
                    },
                );
                native_steps.push(NativeV2StepDraftV1::Search {
                    step_id: step_alias.clone(),
                    depends_on: dependencies.clone(),
                    host_ref: host_ref(&hosts, host_alias)?,
                    output: object,
                    query: query.clone(),
                    safe_scope_labels: safe_scope_labels.clone(),
                });
                topology.push(topology_step(
                    &step_alias,
                    StepOperation::Search,
                    vec![host_alias.clone()],
                    dependencies,
                    None,
                    Some(output_alias.clone()),
                ));
            }
            CandidateSemanticStepV2::Transform {
                host_alias,
                input_alias,
                output_alias,
                modification_intent,
                ..
            } => {
                let input = current_input(&objects, input_alias, &dependencies)?;
                if input.host_alias != *host_alias || objects.contains_key(output_alias) {
                    return invalid(
                        "Natural-v2 Transform would move an object or reuse an object alias.",
                    );
                }
                let output =
                    NativeV2ObjectRevisionDtoV1 {
                        logical_object_id: input.object.logical_object_id.clone(),
                        revision: input.object.revision.checked_add(1).ok_or_else(|| {
                            invalid_error("Natural-v2 object revision overflowed.")
                        })?,
                    };
                replace_current_object(
                    &mut objects,
                    input_alias,
                    output_alias,
                    ObjectState {
                        object: output.clone(),
                        host_alias: host_alias.clone(),
                        producer: Some(step_alias.clone()),
                    },
                )?;
                native_steps.push(NativeV2StepDraftV1::Transform {
                    step_id: step_alias.clone(),
                    depends_on: dependencies.clone(),
                    host_ref: host_ref(&hosts, host_alias)?,
                    input: input.object,
                    output,
                    modification_intent: modification_intent.clone(),
                });
                topology.push(topology_step(
                    &step_alias,
                    StepOperation::Transform,
                    vec![host_alias.clone()],
                    dependencies,
                    Some(input_alias.clone()),
                    Some(output_alias.clone()),
                ));
            }
            CandidateSemanticStepV2::Transfer {
                source_host_alias,
                destination_host_alias,
                input_alias,
                output_alias,
                ..
            } => {
                if !allowed_transfers
                    .contains(&(source_host_alias.clone(), destination_host_alias.clone()))
                {
                    return invalid("Natural-v2 candidate invented an unselected Transfer route.");
                }
                let input = current_input(&objects, input_alias, &dependencies)?;
                if input.host_alias != *source_host_alias
                    || source_host_alias == destination_host_alias
                    || objects.contains_key(output_alias)
                {
                    return invalid(
                        "Natural-v2 Transfer source, destination, or output alias is invalid.",
                    );
                }
                let output = input.object.clone();
                replace_current_object(
                    &mut objects,
                    input_alias,
                    output_alias,
                    ObjectState {
                        object: output.clone(),
                        host_alias: destination_host_alias.clone(),
                        producer: Some(step_alias.clone()),
                    },
                )?;
                native_steps.push(NativeV2StepDraftV1::Transfer {
                    step_id: step_alias.clone(),
                    depends_on: dependencies.clone(),
                    source_host_ref: host_ref(&hosts, source_host_alias)?,
                    destination_host_ref: host_ref(&hosts, destination_host_alias)?,
                    input: input.object,
                    output,
                });
                movements.push(NaturalV2MovementV1 {
                    step_alias: step_alias.clone(),
                    object_alias: output_alias.clone(),
                    source_host_alias: source_host_alias.clone(),
                    destination_host_alias: destination_host_alias.clone(),
                });
                topology.push(topology_step(
                    &step_alias,
                    StepOperation::Transfer,
                    vec![source_host_alias.clone(), destination_host_alias.clone()],
                    dependencies,
                    Some(input_alias.clone()),
                    Some(output_alias.clone()),
                ));
            }
            CandidateSemanticStepV2::Execute {
                host_alias,
                target_alias,
                execution_intent,
                ..
            } => {
                let target = current_input(&objects, target_alias, &dependencies)?;
                if target.host_alias != *host_alias {
                    return invalid("Natural-v2 Execute cannot consume an object at another Host.");
                }
                native_steps.push(NativeV2StepDraftV1::Execute {
                    step_id: step_alias.clone(),
                    depends_on: dependencies.clone(),
                    host_ref: host_ref(&hosts, host_alias)?,
                    target: target.object,
                    execution_intent: execution_intent.clone(),
                });
                topology.push(topology_step(
                    &step_alias,
                    StepOperation::Execute,
                    vec![host_alias.clone()],
                    dependencies,
                    Some(target_alias.clone()),
                    None,
                ));
            }
        }
    }

    let affected_aliases = topology
        .iter()
        .flat_map(|step| step.host_aliases.iter().cloned())
        .chain(
            request
                .candidate
                .roots
                .iter()
                .map(|root| root.host_alias.clone()),
        )
        .collect::<BTreeSet<_>>();
    let affected_hosts = affected_aliases
        .into_iter()
        .map(|alias| {
            let host = hosts
                .get(&alias)
                .ok_or_else(|| invalid_error("Natural-v2 topology names an unknown Host alias."))?;
            Ok(NaturalV2AffectedHostV1 {
                host_alias: alias,
                display_name: host.display_name.clone(),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let compose = NativeV2ComposeRequestV1 {
        plan_id: request.plan_id,
        revision_id: request.revision_id,
        revision_number: request.revision_number,
        bridge_id: request.bridge_id,
        requester_host_ref: requester.host_ref.clone(),
        participant_host_refs: affected_hosts
            .iter()
            .map(|affected| host_ref(&hosts, &affected.host_alias))
            .collect::<AppResult<Vec<_>>>()?,
        roots: native_roots,
        original_user_goal: request.candidate.original_user_goal,
        expected_outcome: request.candidate.expected_outcome,
        steps: native_steps,
    };
    let review = NaturalV2CandidateReviewV1 {
        schema_version: NATURAL_V2_REVIEW_SCHEMA_VERSION.into(),
        title: request.candidate.title,
        draft: NativeV2PlanStatusV1 {
            schema_version: String::new(),
            plan_id: String::new(),
            revision_id: String::new(),
            revision_hash: String::new(),
            approval_id: None,
            attempt_id: None,
            state: crate::native_v2_orchestration::NativeV2ProductStateV1::Draft,
            current_step_id: None,
            completed_steps: 0,
            total_steps: 0,
            ready_hosts: 0,
            total_hosts: 0,
            code: None,
            updated_at: 0,
        },
        affected_hosts,
        topology,
        movements,
    };
    Ok((compose, review))
}

fn validate_request_shape(request: &NaturalV2ComposeCandidateRequestV1) -> AppResult<()> {
    id(&request.plan_id, "plan id")?;
    id(&request.revision_id, "revision id")?;
    id(&request.bridge_id, "Bridge id")?;
    id(&request.requester_host_alias, "requester Host alias")?;
    if request.revision_number == 0
        || request.context.hosts.is_empty()
        || request.context.hosts.len() > MAX_ITEMS
        || request.context.roots.len() > MAX_ITEMS
        || request.candidate.steps.is_empty()
        || request.candidate.steps.len() > MAX_ITEMS
        || request.candidate.roots.len() > MAX_ITEMS
    {
        return invalid("Natural-v2 candidate/context cardinality is invalid.");
    }
    if request.candidate.schema_version != NATURAL_V2_SCHEMA_VERSION {
        return invalid("Natural-v2 candidate schema version is invalid.");
    }
    text(&request.candidate.title, "candidate title")?;
    text(&request.original_user_goal, "Host-captured user goal")?;
    text(&request.candidate.original_user_goal, "user goal")?;
    text(&request.candidate.expected_outcome, "expected outcome")?;
    let mut host_aliases = HashSet::new();
    for host in &request.context.hosts {
        id(&host.alias, "Host alias")?;
        HostRef::parse(host.host_ref.clone())?;
        short_text(&host.display_name, "Host display name")?;
        if !host_aliases.insert(host.alias.as_str()) || host.capability_facts.len() > MAX_ITEMS {
            return invalid(
                "Natural-v2 context contains duplicate Host aliases or too many facts.",
            );
        }
        for fact in &host.capability_facts {
            short_text(fact, "capability fact")?;
        }
    }
    let mut root_aliases = HashSet::new();
    let mut object_aliases = HashSet::new();
    for root in &request.context.roots {
        id(&root.root_alias, "root alias")?;
        id(&root.object_alias, "object alias")?;
        id(&root.logical_object_id, "logical object id")?;
        id(&root.host_alias, "root Host alias")?;
        short_text(&root.display_name, "root display name")?;
        if root.revision == 0
            || !host_aliases.contains(root.host_alias.as_str())
            || !root_aliases.insert(root.root_alias.as_str())
            || !object_aliases.insert(root.object_alias.as_str())
        {
            return invalid("Natural-v2 root selection is duplicate or invalid.");
        }
    }
    if request.context.allowed_operations.is_empty()
        || request.context.allowed_operations.len() > 4
        || request
            .context
            .allowed_operations
            .iter()
            .enumerate()
            .any(|(index, operation)| {
                request.context.allowed_operations[..index].contains(operation)
            })
        || request.context.allowed_scope_labels.len() > MAX_ITEMS
        || request.context.allowed_transfer_routes.len() > MAX_ITEMS
    {
        return invalid("Natural-v2 proposal constraint selection is invalid.");
    }
    for scope in &request.context.allowed_scope_labels {
        id(scope, "scope label")?;
    }
    for route in &request.context.allowed_transfer_routes {
        if !host_aliases.contains(route.source_host_alias.as_str())
            || !host_aliases.contains(route.destination_host_alias.as_str())
            || route.source_host_alias == route.destination_host_alias
        {
            return invalid("Natural-v2 selected Transfer route is invalid.");
        }
    }
    for root in &request.candidate.roots {
        id(&root.root_alias, "candidate root alias")?;
        id(&root.object_alias, "candidate object alias")?;
        id(&root.host_alias, "candidate root Host alias")?;
    }
    for step in &request.candidate.steps {
        id(step.alias(), "step alias")?;
        if step.dependencies().len() > MAX_ITEMS {
            return invalid("Natural-v2 step has too many dependencies.");
        }
        for dependency in step.dependencies() {
            id(dependency, "step dependency")?;
        }
        match step {
            CandidateSemanticStepV2::Search {
                host_alias,
                output_alias,
                query,
                safe_scope_labels,
                ..
            } => {
                id(host_alias, "Search Host alias")?;
                id(output_alias, "Search output alias")?;
                short_text(query, "Search query")?;
                if query.len() > 128 || safe_scope_labels.len() > MAX_ITEMS {
                    return invalid("Natural-v2 Search query/scopes are invalid.");
                }
            }
            CandidateSemanticStepV2::Transform {
                host_alias,
                input_alias,
                output_alias,
                modification_intent,
                ..
            } => {
                id(host_alias, "Transform Host alias")?;
                id(input_alias, "Transform input alias")?;
                id(output_alias, "Transform output alias")?;
                text(modification_intent, "Transform intent")?;
            }
            CandidateSemanticStepV2::Transfer {
                source_host_alias,
                destination_host_alias,
                input_alias,
                output_alias,
                ..
            } => {
                id(source_host_alias, "Transfer source Host alias")?;
                id(destination_host_alias, "Transfer destination Host alias")?;
                id(input_alias, "Transfer input alias")?;
                id(output_alias, "Transfer output alias")?;
            }
            CandidateSemanticStepV2::Execute {
                host_alias,
                target_alias,
                execution_intent,
                ..
            } => {
                id(host_alias, "Execute Host alias")?;
                id(target_alias, "Execute target alias")?;
                text(execution_intent, "Execute intent")?;
            }
        }
    }
    Ok(())
}

fn current_input(
    objects: &HashMap<String, ObjectState>,
    alias: &str,
    dependencies: &[String],
) -> AppResult<ObjectState> {
    let state = objects.get(alias).cloned().ok_or_else(|| {
        invalid_error("Natural-v2 step consumes an unknown or stale object alias.")
    })?;
    if state
        .producer
        .as_ref()
        .is_some_and(|producer| !dependencies.contains(producer))
    {
        return invalid("Natural-v2 step does not depend on the exact object producer.");
    }
    Ok(state)
}

fn replace_current_object(
    objects: &mut HashMap<String, ObjectState>,
    input_alias: &str,
    output_alias: &str,
    state: ObjectState,
) -> AppResult<()> {
    if input_alias == output_alias || objects.remove(input_alias).is_none() {
        return invalid("Natural-v2 object flow aliases must advance explicitly.");
    }
    if objects.insert(output_alias.to_string(), state).is_some() {
        return invalid("Natural-v2 object flow reused an existing alias.");
    }
    Ok(())
}

fn host_ref(hosts: &BTreeMap<String, NaturalV2HostSelectionV1>, alias: &str) -> AppResult<String> {
    hosts
        .get(alias)
        .map(|host| host.host_ref.clone())
        .ok_or_else(|| invalid_error("Natural-v2 candidate names an unknown Host alias."))
}

fn topology_step(
    step_alias: &str,
    operation: StepOperation,
    host_aliases: Vec<String>,
    depends_on: Vec<String>,
    input_alias: Option<String>,
    output_alias: Option<String>,
) -> NaturalV2TopologyStepV1 {
    NaturalV2TopologyStepV1 {
        step_alias: step_alias.into(),
        operation,
        host_aliases,
        depends_on,
        input_alias,
        output_alias,
    }
}

fn derived_search_object_id(plan_id: &str, revision_id: &str, step_alias: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-natural-v2-search-object-v1\0");
    hasher.update(plan_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(revision_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(step_alias.as_bytes());
    format!("managed-object:v1:{}", hasher.finalize().to_hex())
}

fn id(value: &str, label: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_ID
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return invalid(&format!("Natural-v2 {label} is invalid."));
    }
    Ok(())
}

fn text(value: &str, label: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > MAX_TEXT {
        return invalid(&format!("Natural-v2 {label} is invalid."));
    }
    Ok(())
}

fn short_text(value: &str, label: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > MAX_FACT {
        return invalid(&format!("Natural-v2 {label} is invalid."));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_v2_orchestration::compose_revision;

    fn host(label: &str) -> String {
        HostRef::from_device_id(label).unwrap().as_str().to_string()
    }

    fn base_request() -> NaturalV2ComposeCandidateRequestV1 {
        NaturalV2ComposeCandidateRequestV1 {
            plan_id: "plan-natural-v2".into(),
            revision_id: "revision-natural-v2".into(),
            revision_number: 1,
            bridge_id: "bridge-natural-v2".into(),
            requester_host_alias: "host_a".into(),
            original_user_goal: "Update the document on B and run it on C.".into(),
            context: NaturalV2ProposalContextV1 {
                hosts: ["a", "b", "c"]
                    .into_iter()
                    .map(|name| NaturalV2HostSelectionV1 {
                        alias: format!("host_{name}"),
                        host_ref: host(name),
                        display_name: format!("Host {name}"),
                        capability_facts: vec!["observed_transform_available".into()],
                    })
                    .collect(),
                roots: vec![NaturalV2RootSelectionV1 {
                    root_alias: "root_document".into(),
                    object_alias: "document_n".into(),
                    logical_object_id: "managed-object:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                    revision: 7,
                    host_alias: "host_b".into(),
                    display_name: "document.txt".into(),
                }],
                allowed_operations: vec![StepOperation::Search, StepOperation::Transform, StepOperation::Transfer, StepOperation::Execute],
                allowed_transfer_routes: vec![NaturalV2TransferSelectionV1 {
                    source_host_alias: "host_b".into(),
                    destination_host_alias: "host_c".into(),
                }],
                allowed_scope_labels: vec!["documents".into()],
            },
            candidate: CandidateSemanticPlanV2 {
                schema_version: NATURAL_V2_SCHEMA_VERSION.into(),
                title: "Transform, move, and execute".into(),
                original_user_goal: "Update the document on B and run it on C.".into(),
                expected_outcome: "The reviewed revision is executed on C.".into(),
                roots: vec![CandidateSemanticRootV2 {
                    root_alias: "root_document".into(),
                    object_alias: "document_n".into(),
                    host_alias: "host_b".into(),
                }],
                steps: vec![
                    CandidateSemanticStepV2::Transform {
                        step_alias: "transform_document".into(), depends_on: vec![], host_alias: "host_b".into(), input_alias: "document_n".into(), output_alias: "document_n1".into(), modification_intent: "Apply the reviewed update.".into(),
                    },
                    CandidateSemanticStepV2::Transfer {
                        step_alias: "transfer_document".into(), depends_on: vec!["transform_document".into()], source_host_alias: "host_b".into(), destination_host_alias: "host_c".into(), input_alias: "document_n1".into(), output_alias: "document_n1_at_c".into(),
                    },
                    CandidateSemanticStepV2::Execute {
                        step_alias: "execute_document".into(), depends_on: vec!["transfer_document".into()], host_alias: "host_c".into(), target_alias: "document_n1_at_c".into(), execution_intent: "Run the reviewed validation.".into(),
                    },
                ],
            },
        }
    }

    #[test]
    fn generic_root_lowers_to_exact_n_plus_one_transfer_and_execute() {
        let (compose, review) = lower_candidate(base_request()).unwrap();
        let revision = compose_revision(compose).unwrap();
        assert_eq!(revision.roots[0].object.revision, 7);
        assert_eq!(review.movements.len(), 1);
        assert_eq!(review.affected_hosts.len(), 2);
        match &revision.steps[0] {
            crate::bridge_plan_v2::PlanStepV2::Transform { input, output, .. } => {
                assert_eq!(input.revision, 7);
                assert_eq!(output.revision, 8);
                assert_eq!(input.logical_object_id, output.logical_object_id);
            }
            _ => panic!("expected Transform"),
        }
        match &revision.steps[2] {
            crate::bridge_plan_v2::PlanStepV2::Execute { target, .. } => {
                assert_eq!(target.revision, 8)
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn explicit_transfer_is_the_only_location_change() {
        let mut request = base_request();
        if let CandidateSemanticStepV2::Transform { host_alias, .. } =
            &mut request.candidate.steps[0]
        {
            *host_alias = "host_c".into();
        }
        assert!(lower_candidate(request)
            .unwrap_err()
            .message()
            .contains("move"));

        let mut request = base_request();
        request.context.allowed_transfer_routes.clear();
        assert!(lower_candidate(request)
            .unwrap_err()
            .message()
            .contains("unselected Transfer"));
    }

    #[test]
    fn stale_alias_dependency_and_revision_flow_fail_closed() {
        let mut unknown_host = base_request();
        if let CandidateSemanticStepV2::Execute { host_alias, .. } =
            &mut unknown_host.candidate.steps[2]
        {
            *host_alias = "fabricated_host".into();
        }
        assert!(lower_candidate(unknown_host).is_err());

        let mut missing_dependency = base_request();
        if let CandidateSemanticStepV2::Transfer { depends_on, .. } =
            &mut missing_dependency.candidate.steps[1]
        {
            depends_on.clear();
        }
        assert!(lower_candidate(missing_dependency)
            .unwrap_err()
            .message()
            .contains("exact object producer"));
    }

    #[test]
    fn capability_facts_do_not_authorize_operations_or_transfer() {
        let mut request = base_request();
        request.context.allowed_operations = vec![StepOperation::Transform];
        for host in &mut request.context.hosts {
            host.capability_facts = vec!["transfer_and_execute_available".into()];
        }
        assert!(lower_candidate(request)
            .unwrap_err()
            .message()
            .contains("outside the selected"));
    }

    #[test]
    fn search_identity_is_core_derived_and_deterministic() {
        let mut request = base_request();
        request.context.roots.clear();
        request.candidate.roots.clear();
        request.candidate.steps = vec![CandidateSemanticStepV2::Search {
            step_alias: "search_report".into(),
            depends_on: vec![],
            host_alias: "host_b".into(),
            output_alias: "report_n".into(),
            query: "report.txt".into(),
            safe_scope_labels: vec!["documents".into()],
        }];
        let (first, _) = lower_candidate(request.clone()).unwrap();
        let (second, _) = lower_candidate(request).unwrap();
        assert_eq!(first, second);
        match &first.steps[0] {
            NativeV2StepDraftV1::Search { output, .. } => {
                assert!(output.logical_object_id.starts_with("managed-object:v1:"));
                assert_eq!(output.revision, 1);
            }
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn candidate_review_has_no_approval_attempt_or_effect_authority() {
        let (_, review) = lower_candidate(base_request()).unwrap();
        let json = serde_json::to_string(&review).unwrap();
        assert!(!json.contains("effectEnvelope"));
        assert!(!json.contains("networkGrant"));
        assert!(!json.contains("secretHandle"));
        assert!(review.draft.approval_id.is_none());
        assert!(review.draft.attempt_id.is_none());
    }

    #[test]
    fn host_captured_goal_cannot_be_replaced_by_candidate_output() {
        let mut request = base_request();
        request.candidate.original_user_goal = "A provider replacement goal.".into();
        assert!(lower_candidate(request)
            .unwrap_err()
            .message()
            .contains("Host-captured user goal"));
    }
}

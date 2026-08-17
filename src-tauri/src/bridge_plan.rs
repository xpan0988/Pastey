//! Durable, Host-owned Bridge Plan foundation.
//!
//! This module intentionally has no Tauri command and is not connected to the
//! current Ask Bridge UI or TaskGraph executor.  It stores safe workspace
//! history only; all capability grants, ObjectRef backing, leases, and process
//! state remain in their existing ephemeral Host-owned stores.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    models::PipelineHandoffMetadata,
    storage::AppPaths,
};

mod protocol;
pub(crate) use protocol::{
    accept_inbound_protocol_event, attempt_search_result_payload, attempt_start_payload,
    attempt_update_payload, consume_search_execution_grant, consume_transfer_execution_grant,
    protocol_metadata, reconcile_protocol_startup, record_outbound_protocol_event,
    review_request_payload, search_selection_payload, transfer_start_payload,
    transfer_update_payload, ProtocolSearchAuthorityStore,
};

const HASH_VERSION: &str = "bridge-plan-revision-hash-v1";
const MAX_ID_LEN: usize = 128;
const MAX_TEXT_LEN: usize = 1_024;
const MAX_STEPS: usize = 16;
const MAX_SLOTS_PER_STEP: usize = 16;
const MAX_MEDIA_TYPES: usize = 16;
const MAX_DEPENDENCIES: usize = 16;
const MAX_PLANS_PER_BRIDGE: i64 = 128;
const MAX_REVISIONS_PER_PLAN: i64 = 64;
const MAX_APPROVALS_PER_REVISION: i64 = 128;
const MAX_ATTEMPTS_PER_REVISION: i64 = 256;
const MAX_ACTIVITIES_PER_PLAN: i64 = 1_024;
const MAX_RESULTS_PER_ATTEMPT: i64 = 128;
const MAX_SAFE_SCOPE_LABELS: usize = 16;
const MAX_CAPABILITY_REQUIREMENTS: usize = 16;
const MAX_GRAPH_DEPENDENCIES: usize = 16;

macro_rules! durable_text {
    ($name:ident) => {
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
        #[serde(transparent)]
        pub(crate) struct $name(String);
        impl $name {
            fn validate(&self, field: &str) -> AppResult<()> {
                validate_bounded_text(&self.0, field)
            }
        }
    };
}

durable_text!(RawUserGoal);
durable_text!(GeneratedUserVisibleText);
durable_text!(SafeLocationDescription);
durable_text!(SafeActivitySummary);

macro_rules! durable_text_as_str {
    ($name:ident) => {
        impl $name {
            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

durable_text_as_str!(GeneratedUserVisibleText);
durable_text_as_str!(SafeLocationDescription);

impl GeneratedUserVisibleText {
    pub(crate) fn from_semantic(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[cfg(test)]
impl From<&str> for GeneratedUserVisibleText {
    fn from(value: &str) -> Self {
        Self::from_semantic(value)
    }
}

macro_rules! bounded_text_from {
    ($name:ident) => {
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

bounded_text_from!(RawUserGoal);
bounded_text_from!(SafeLocationDescription);
bounded_text_from!(SafeActivitySummary);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BridgePlanState {
    Draft,
    Open,
    Cancelled,
    Burned,
}
impl BridgePlanState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Cancelled => "cancelled",
            Self::Burned => "burned",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RevisionState {
    Proposed,
    Available,
    Superseded,
    Withdrawn,
    Burned,
}
impl RevisionState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Available => "available",
            Self::Superseded => "superseded",
            Self::Withdrawn => "withdrawn",
            Self::Burned => "burned",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalState {
    AwaitingReceiver,
    Valid,
    Denied,
    Expired,
    Consumed,
    Revoked,
    Burned,
}

impl ApprovalState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingReceiver => "awaiting_receiver",
            Self::Valid => "valid",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Consumed => "consumed",
            Self::Revoked => "revoked",
            Self::Burned => "burned",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptState {
    Created,
    Running,
    Interrupted,
    Completed,
    Failed,
    Cancelled,
    Burned,
}
impl AttemptState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Interrupted => "interrupted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Burned => "burned",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepOperation {
    Search,
    Transform,
    Transfer,
    Execute,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SlotCardinality {
    One,
    Many,
    Optional,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepFailureBehavior {
    StopPlan,
    RequireNewRevision,
    AwaitBoundedChoice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchSelectionMode {
    BoundedInline,
    Staged,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActivityKind {
    RevisionProposed,
    ApprovalCreated,
    ReceiverAuthorized,
    AttemptCreated,
    AttemptStarted,
    AttemptInterrupted,
    AttemptCompleted,
    AttemptFailed,
    AttemptCancelled,
    ResultRecorded,
    AlternativeProposed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlan {
    pub(crate) plan_id: String,
    pub(crate) bridge_id: String,
    pub(crate) requesting_device_ref: String,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanPresentation {
    pub(crate) title: GeneratedUserVisibleText,
    pub(crate) natural_language_plan: GeneratedUserVisibleText,
    pub(crate) step_explanations: Vec<StepExplanation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StepExplanation {
    pub(crate) step_id: String,
    pub(crate) action_summary: GeneratedUserVisibleText,
    pub(crate) expected_result: GeneratedUserVisibleText,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ObjectContract {
    pub(crate) object_type: GeneratedUserVisibleText,
    pub(crate) media_types: Vec<GeneratedUserVisibleText>,
    pub(crate) user_visible_description: GeneratedUserVisibleText,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PlanSlot {
    pub(crate) slot_id: String,
    pub(crate) object: ObjectContract,
    pub(crate) cardinality: SlotCardinality,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum ObjectSelectionRule {
    FromSlot {
        slot_id: String,
    },
    FutureUserSelection {
        object: ObjectContract,
        selection_prompt: GeneratedUserVisibleText,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum TransferDestination {
    /// A private, expiring object handoff required by a following step. It is
    /// distinct from Inbox/Pastey Shared final delivery in the revision.
    PipelineHandoff {
        device_ref: String,
    },
    RequestingDevice {
        device_ref: String,
    },
    SelectedDevice {
        device_ref: String,
    },
    UserSelectedLocation {
        device_ref: String,
        user_visible_location_scope: SafeLocationDescription,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CapabilityRequirement {
    pub(crate) category: GeneratedUserVisibleText,
    pub(crate) user_visible_requirement: GeneratedUserVisibleText,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BoundedSearchSelectionRule {
    pub(crate) source_slot_id: String,
    pub(crate) result_set_limit: u16,
    pub(crate) allowed_object: ObjectContract,
    pub(crate) downstream_slot_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SearchIntent {
    pub(crate) query: GeneratedUserVisibleText,
    pub(crate) extensions: Vec<GeneratedUserVisibleText>,
    pub(crate) safe_scope_labels: Vec<SafeLocationDescription>,
}

/// A semantic dependency in the immutable Plan. It identifies the logical
/// object revision a framework-only action consumes or would produce; it is
/// not an ObjectRef, filesystem identity, capability grant, or claim that the
/// future Agent action has executed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LogicalObjectRevision {
    pub(crate) logical_object_id: String,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "operation")]
pub(crate) enum BridgePlanStep {
    Search {
        step_id: String,
        depends_on: Vec<String>,
        input_slots: Vec<PlanSlot>,
        output_slots: Vec<PlanSlot>,
        source_device_ref: Option<String>,
        execution_device_ref: String,
        user_visible_action: GeneratedUserVisibleText,
        capability_requirements: Vec<CapabilityRequirement>,
        failure_behavior: StepFailureBehavior,
        query: SearchIntent,
        selection: Option<BoundedSearchSelectionRule>,
    },
    Transform {
        step_id: String,
        depends_on: Vec<String>,
        input_slots: Vec<PlanSlot>,
        output_slots: Vec<PlanSlot>,
        source_device_ref: Option<String>,
        execution_device_ref: String,
        user_visible_action: GeneratedUserVisibleText,
        capability_requirements: Vec<CapabilityRequirement>,
        failure_behavior: StepFailureBehavior,
        target: ObjectSelectionRule,
        input_revision: LogicalObjectRevision,
        output_revision: LogicalObjectRevision,
        modification_intent: RawUserGoal,
        expected_input: ObjectContract,
    },
    Transfer {
        step_id: String,
        depends_on: Vec<String>,
        input_slots: Vec<PlanSlot>,
        output_slots: Vec<PlanSlot>,
        source_device_ref: Option<String>,
        execution_device_ref: String,
        user_visible_action: GeneratedUserVisibleText,
        capability_requirements: Vec<CapabilityRequirement>,
        failure_behavior: StepFailureBehavior,
        source: ObjectSelectionRule,
        destination: TransferDestination,
    },
    Execute {
        step_id: String,
        depends_on: Vec<String>,
        input_slots: Vec<PlanSlot>,
        output_slots: Vec<PlanSlot>,
        source_device_ref: Option<String>,
        execution_device_ref: String,
        user_visible_action: GeneratedUserVisibleText,
        capability_requirements: Vec<CapabilityRequirement>,
        failure_behavior: StepFailureBehavior,
        target: ObjectSelectionRule,
        target_revision: LogicalObjectRevision,
        execution_intent: RawUserGoal,
    },
}

impl BridgePlanStep {
    fn id(&self) -> &str {
        match self {
            Self::Search { step_id, .. }
            | Self::Transform { step_id, .. }
            | Self::Transfer { step_id, .. }
            | Self::Execute { step_id, .. } => step_id,
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
    fn inputs(&self) -> &[PlanSlot] {
        match self {
            Self::Search { input_slots, .. }
            | Self::Transform { input_slots, .. }
            | Self::Transfer { input_slots, .. }
            | Self::Execute { input_slots, .. } => input_slots,
        }
    }
    fn outputs(&self) -> &[PlanSlot] {
        match self {
            Self::Search { output_slots, .. }
            | Self::Transform { output_slots, .. }
            | Self::Transfer { output_slots, .. }
            | Self::Execute { output_slots, .. } => output_slots,
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
    fn execution_device(&self) -> &str {
        match self {
            Self::Search {
                execution_device_ref,
                ..
            }
            | Self::Transform {
                execution_device_ref,
                ..
            }
            | Self::Transfer {
                execution_device_ref,
                ..
            }
            | Self::Execute {
                execution_device_ref,
                ..
            } => execution_device_ref,
        }
    }
    fn source_device(&self) -> Option<&str> {
        match self {
            Self::Search {
                source_device_ref, ..
            }
            | Self::Transform {
                source_device_ref, ..
            }
            | Self::Transfer {
                source_device_ref, ..
            }
            | Self::Execute {
                source_device_ref, ..
            } => source_device_ref.as_deref(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AlternativeProposal {
    pub(crate) based_on_revision_id: String,
    pub(crate) change_explanation: GeneratedUserVisibleText,
}

/// Semantic payload. Storage state, IDs, timestamps, and the resulting hash are
/// deliberately outside this value so retries and storage metadata cannot alter
/// task meaning.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanRevision {
    pub(crate) schema_version: GeneratedUserVisibleText,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_number: u32,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) requesting_device_ref: String,
    pub(crate) selected_device_ref: String,
    pub(crate) original_user_goal: RawUserGoal,
    pub(crate) presentation: BridgePlanPresentation,
    pub(crate) expected_outcome: GeneratedUserVisibleText,
    pub(crate) search_selection_mode: SearchSelectionMode,
    pub(crate) steps: Vec<BridgePlanStep>,
    pub(crate) alternative: Option<AlternativeProposal>,
}

/// Bounded semantic blocks accepted from the guided Composer after command
/// code resolves Bridge roles to current-session device references. These are
/// product primitives, not execution grants or renderer-provided ObjectRefs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposedFilePlanBlock {
    Search {
        execution_device_ref: String,
        filename_hint: String,
        extensions: Vec<String>,
        safe_scope_labels: Vec<String>,
    },
    Transform {
        execution_device_ref: String,
        target_revision: LogicalObjectRevision,
        modification_intent: String,
    },
    Transfer {
        source_device_ref: String,
        destination_device_ref: String,
        landing: ComposedTransferLanding,
    },
    Execute {
        execution_device_ref: String,
        target_revision: LogicalObjectRevision,
        execution_intent: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposedTransferLanding {
    PipelinePrivate,
    Inbox,
    PasteyShared,
}

/// Builds the only currently executable file-search revision from bounded
/// product intent. The renderer never supplies a revision, object reference,
/// device binding, or execution grant.
#[cfg(test)]
pub(crate) fn build_file_search_revision(
    bridge_id: String,
    requesting_device_ref: String,
    selected_device_ref: String,
    original_user_goal: String,
    filename_hint: String,
    extensions: Vec<String>,
    safe_scope_labels: Vec<String>,
) -> AppResult<BridgePlanRevision> {
    build_file_plan_revision(
        bridge_id,
        requesting_device_ref,
        selected_device_ref,
        original_user_goal,
        filename_hint,
        extensions,
        safe_scope_labels,
        false,
    )
}

/// Builds a direct requester-to-selected-device file Transfer. The user picks
/// the local source outside the revision; only this bounded future-selection
/// contract is durable, while the Host keeps the canonical path ephemeral.
pub(crate) fn build_direct_file_transfer_revision(
    bridge_id: String,
    requesting_device_ref: String,
    selected_device_ref: String,
    original_user_goal: String,
) -> AppResult<BridgePlanRevision> {
    let object = ObjectContract {
        object_type: GeneratedUserVisibleText::from_semantic("file"),
        media_types: Vec::new(),
        user_visible_description: GeneratedUserVisibleText::from_semantic(
            "one file chosen on the requesting device",
        ),
    };
    let transfer = BridgePlanStep::Transfer {
        step_id: "transfer".into(),
        depends_on: Vec::new(),
        input_slots: Vec::new(),
        output_slots: Vec::new(),
        source_device_ref: Some(requesting_device_ref.clone()),
        execution_device_ref: requesting_device_ref.clone(),
        user_visible_action: GeneratedUserVisibleText::from_semantic(
            "Transfer the file chosen on the requesting device to the selected device.",
        ),
        capability_requirements: vec![CapabilityRequirement {
            category: GeneratedUserVisibleText::from_semantic("file_transfer"),
            user_visible_requirement: GeneratedUserVisibleText::from_semantic(
                "Send only the single file chosen for this reviewed plan.",
            ),
        }],
        failure_behavior: StepFailureBehavior::StopPlan,
        source: ObjectSelectionRule::FutureUserSelection {
            object,
            selection_prompt: GeneratedUserVisibleText::from_semantic(
                "Choose one local file to transfer after the complete plan is approved.",
            ),
        },
        destination: TransferDestination::SelectedDevice {
            device_ref: selected_device_ref.clone(),
        },
    };
    let mut revision = BridgePlanRevision {
        schema_version: GeneratedUserVisibleText::from_semantic("bridge-plan-v1"),
        plan_id: format!("plan-{}", uuid::Uuid::new_v4()),
        revision_id: format!("revision-{}", uuid::Uuid::new_v4()),
        revision_number: 1,
        revision_hash: String::new(),
        bridge_id,
        requesting_device_ref,
        selected_device_ref,
        original_user_goal: RawUserGoal::from(original_user_goal),
        presentation: BridgePlanPresentation {
            title: GeneratedUserVisibleText::from_semantic("Transfer a file to the selected device"),
            natural_language_plan: GeneratedUserVisibleText::from_semantic(
                "After one requester Review & Run, choose one file on the requesting device and transfer it to the selected device.",
            ),
            step_explanations: vec![StepExplanation {
                step_id: "transfer".into(),
                action_summary: GeneratedUserVisibleText::from_semantic("Transfer one chosen file."),
                expected_result: GeneratedUserVisibleText::from_semantic("The selected device receives the chosen file through the Bridge transfer channel."),
            }],
        },
        expected_outcome: GeneratedUserVisibleText::from_semantic(
            "One file chosen after approval is transferred to the selected device.",
        ),
        search_selection_mode: SearchSelectionMode::Staged,
        steps: vec![transfer],
        alternative: None,
    };
    validate_revision(&revision)?;
    revision.revision_hash = canonical_revision_hash(&revision)?;
    Ok(revision)
}

/// Lowers the exact authored block sequence into one immutable revision.
/// Object locations are advanced only by explicit Transfer blocks; the final
/// validator independently rejects wrong-device consumption and cycles.
fn invalid_revision(message: &str) -> AppResult<BridgePlanRevision> {
    Err(AppError::InvalidInput(message.into()))
}

pub(crate) fn build_composed_file_revision(
    bridge_id: String,
    requesting_device_ref: String,
    selected_device_ref: String,
    original_user_goal: String,
    blocks: Vec<ComposedFilePlanBlock>,
) -> AppResult<BridgePlanRevision> {
    const ALLOWED_SCOPES: &[&str] = &["downloads", "desktop", "documents", "pastey_shared"];
    if blocks.is_empty() || blocks.len() > MAX_STEPS {
        return invalid_revision("The composed Bridge Plan has an invalid number of blocks.");
    }
    if matches!(
        blocks.last(),
        Some(ComposedFilePlanBlock::Transfer {
            landing: ComposedTransferLanding::PipelinePrivate,
            ..
        })
    ) {
        return invalid_revision(
            "A private pipeline Transfer needs a following step that consumes its object.",
        );
    }
    let matches_bridge =
        |device: &str| device == requesting_device_ref || device == selected_device_ref;
    let downstream_exists = blocks.len() > 1;
    let mut steps = Vec::with_capacity(blocks.len());
    let mut explanations = Vec::with_capacity(blocks.len());
    let mut current_slot: Option<(String, ObjectContract)> = None;
    let mut current_device: Option<String> = None;
    let mut current_revision: Option<u64> = None;
    let mut previous_step: Option<String> = None;

    for (index, block) in blocks.into_iter().enumerate() {
        let step_id = match &block {
            ComposedFilePlanBlock::Search { .. } => "search".to_owned(),
            ComposedFilePlanBlock::Transform { .. } => format!("transform-{index}"),
            ComposedFilePlanBlock::Transfer { .. } => format!("transfer-{index}"),
            ComposedFilePlanBlock::Execute { .. } => format!("execute-{index}"),
        };
        let depends_on = previous_step.iter().cloned().collect::<Vec<_>>();
        match block {
            ComposedFilePlanBlock::Search {
                execution_device_ref,
                filename_hint,
                extensions,
                safe_scope_labels,
            } => {
                if index != 0 || current_slot.is_some() || !matches_bridge(&execution_device_ref) {
                    return invalid_revision(
                        "Search must start the composed object flow on a current Bridge device.",
                    );
                }
                if safe_scope_labels.is_empty()
                    || safe_scope_labels
                        .iter()
                        .any(|scope| !ALLOWED_SCOPES.contains(&scope.as_str()))
                {
                    return invalid_revision(
                        "Bridge Plan Search must use one or more supported safe locations.",
                    );
                }
                let query = filename_hint.trim();
                if query.is_empty() {
                    return invalid_revision(
                        "Bridge Plan Search needs a filename or file description.",
                    );
                }
                let extensions = extensions
                    .into_iter()
                    .map(|extension| {
                        GeneratedUserVisibleText::from_semantic(extension.to_ascii_lowercase())
                    })
                    .collect::<Vec<_>>();
                let object = ObjectContract {
                    object_type: GeneratedUserVisibleText::from_semantic("file"),
                    media_types: Vec::new(),
                    user_visible_description: GeneratedUserVisibleText::from_semantic(
                        "the selected matching file",
                    ),
                };
                let selection = downstream_exists.then(|| BoundedSearchSelectionRule {
                    source_slot_id: "found".into(),
                    result_set_limit: 10,
                    allowed_object: object.clone(),
                    downstream_slot_id: "selected_file".into(),
                });
                steps.push(BridgePlanStep::Search {
                    step_id: step_id.clone(),
                    depends_on,
                    input_slots: Vec::new(),
                    output_slots: vec![PlanSlot {
                        slot_id: "found".into(),
                        object: object.clone(),
                        cardinality: SlotCardinality::Many,
                    }],
                    source_device_ref: Some(execution_device_ref.clone()),
                    execution_device_ref: execution_device_ref.clone(),
                    user_visible_action: GeneratedUserVisibleText::from_semantic(
                        "Search the approved device and reviewed locations for matching files.",
                    ),
                    capability_requirements: vec![CapabilityRequirement {
                        category: GeneratedUserVisibleText::from_semantic("object_search"),
                        user_visible_requirement: GeneratedUserVisibleText::from_semantic(
                            "Search only the reviewed locations.",
                        ),
                    }],
                    failure_behavior: StepFailureBehavior::StopPlan,
                    query: SearchIntent {
                        query: GeneratedUserVisibleText::from_semantic(query),
                        extensions,
                        safe_scope_labels: safe_scope_labels
                            .into_iter()
                            .map(SafeLocationDescription::from)
                            .collect(),
                    },
                    selection,
                });
                if downstream_exists {
                    current_slot = Some(("selected_file".into(), object));
                    current_device = Some(execution_device_ref.clone());
                    current_revision = Some(1);
                }
                explanations.push(StepExplanation {
                    step_id: step_id.clone(),
                    action_summary: GeneratedUserVisibleText::from_semantic(format!(
                        "Search on {execution_device_ref}."
                    )),
                    expected_result: GeneratedUserVisibleText::from_semantic(
                        if downstream_exists {
                            "One bounded result is selected as the next step's input."
                        } else {
                            "A bounded summary of matching files is returned."
                        },
                    ),
                });
            }
            ComposedFilePlanBlock::Transform {
                execution_device_ref,
                target_revision,
                modification_intent,
            } => {
                if !matches_bridge(&execution_device_ref) {
                    return invalid_revision(
                        "The composed Transform execution device is outside this Bridge.",
                    );
                }
                let (input_slot, input_contract) = current_slot.clone().ok_or_else(|| {
                    AppError::InvalidInput("Transform needs an input object.".into())
                })?;
                if current_device.as_deref() != Some(execution_device_ref.as_str()) {
                    return invalid_revision("Transform input is not local to its approved execution device; add an explicit Transfer first.");
                }
                if current_revision != Some(target_revision.revision)
                    || target_revision.logical_object_id != "selected_file"
                {
                    return invalid_revision(
                        "Transform must consume the current logical object revision.",
                    );
                }
                let modification_intent = RawUserGoal::from(modification_intent);
                modification_intent.validate("Transform modification intent")?;
                let output_revision = LogicalObjectRevision {
                    logical_object_id: target_revision.logical_object_id.clone(),
                    revision: target_revision.revision.checked_add(1).ok_or_else(|| {
                        AppError::InvalidInput("Transform revision overflowed.".into())
                    })?,
                };
                let output_slot_id = format!("transformed_file_{index}");
                let output_contract = ObjectContract {
                    object_type: input_contract.object_type.clone(),
                    media_types: input_contract.media_types.clone(),
                    user_visible_description: GeneratedUserVisibleText::from_semantic(
                        "the same logical file after the reviewed modification intent",
                    ),
                };
                steps.push(BridgePlanStep::Transform {
                    step_id: step_id.clone(),
                    depends_on,
                    input_slots: vec![PlanSlot {
                        slot_id: input_slot.clone(),
                        object: input_contract.clone(),
                        cardinality: SlotCardinality::One,
                    }],
                    output_slots: vec![PlanSlot {
                        slot_id: output_slot_id.clone(),
                        object: output_contract.clone(),
                        cardinality: SlotCardinality::One,
                    }],
                    source_device_ref: Some(execution_device_ref.clone()),
                    execution_device_ref: execution_device_ref.clone(),
                    user_visible_action: GeneratedUserVisibleText::from_semantic(format!(
                        "Authorize the reviewed modification intent on {execution_device_ref}."
                    )),
                    capability_requirements: Vec::new(),
                    failure_behavior: StepFailureBehavior::RequireNewRevision,
                    target: ObjectSelectionRule::FromSlot {
                        slot_id: input_slot,
                    },
                    input_revision: target_revision,
                    output_revision: output_revision.clone(),
                    modification_intent,
                    expected_input: input_contract,
                });
                current_slot = Some((output_slot_id, output_contract));
                current_device = Some(execution_device_ref.clone());
                current_revision = Some(output_revision.revision);
                explanations.push(StepExplanation {
                    step_id: step_id.clone(),
                    action_summary: GeneratedUserVisibleText::from_semantic(format!(
                        "Modify the selected file on {execution_device_ref}."
                    )),
                    expected_result: GeneratedUserVisibleText::from_semantic(
                        "If a future Agent implementation performs the reviewed intent, the same logical file advances one revision without moving. This step is not currently executable.",
                    ),
                });
            }
            ComposedFilePlanBlock::Transfer {
                source_device_ref,
                destination_device_ref,
                landing,
            } => {
                if !matches_bridge(&source_device_ref) || !matches_bridge(&destination_device_ref) {
                    return invalid_revision(
                        "The composed Transfer references a device outside its current Bridge.",
                    );
                }
                if current_device.as_deref() != Some(source_device_ref.as_str()) {
                    return invalid_revision(
                        "Transfer source does not own the current input object.",
                    );
                }
                let (input_slot, input_contract) = current_slot.clone().ok_or_else(|| {
                    AppError::InvalidInput("Transfer needs an input object.".into())
                })?;
                if source_device_ref == destination_device_ref
                    && landing == ComposedTransferLanding::PipelinePrivate
                {
                    return invalid_revision(
                        "A private pipeline Transfer must move the object to another device.",
                    );
                }
                if landing == ComposedTransferLanding::PasteyShared
                    && source_device_ref != selected_device_ref
                {
                    return invalid_revision(
                        "Pastey Shared final delivery requires the object to be on the selected device.",
                    );
                }
                let pipeline = landing == ComposedTransferLanding::PipelinePrivate;
                let output_slot_id = format!("pipeline_file_{index}");
                let destination = match landing {
                    ComposedTransferLanding::PipelinePrivate => {
                        TransferDestination::PipelineHandoff {
                            device_ref: destination_device_ref.clone(),
                        }
                    }
                    ComposedTransferLanding::Inbox
                        if destination_device_ref == requesting_device_ref =>
                    {
                        TransferDestination::RequestingDevice {
                            device_ref: destination_device_ref.clone(),
                        }
                    }
                    ComposedTransferLanding::Inbox => TransferDestination::SelectedDevice {
                        device_ref: destination_device_ref.clone(),
                    },
                    ComposedTransferLanding::PasteyShared
                        if destination_device_ref == selected_device_ref =>
                    {
                        TransferDestination::UserSelectedLocation {
                            device_ref: destination_device_ref.clone(),
                            user_visible_location_scope: SafeLocationDescription::from(
                                "Pastey Shared",
                            ),
                        }
                    }
                    ComposedTransferLanding::PasteyShared => {
                        return invalid_revision(
                            "Pastey Shared is available only on the selected device.",
                        )
                    }
                };
                steps.push(BridgePlanStep::Transfer {
                    step_id: step_id.clone(),
                    depends_on,
                    input_slots: vec![PlanSlot { slot_id: input_slot.clone(), object: input_contract.clone(), cardinality: SlotCardinality::One }],
                    output_slots: pipeline.then(|| PlanSlot { slot_id: output_slot_id.clone(), object: input_contract.clone(), cardinality: SlotCardinality::One }).into_iter().collect(),
                    source_device_ref: Some(source_device_ref.clone()),
                    execution_device_ref: source_device_ref.clone(),
                    user_visible_action: GeneratedUserVisibleText::from_semantic(if pipeline {
                        format!("Transfer the current object privately from {source_device_ref} to {destination_device_ref} for the next approved step.")
                    } else {
                        format!("Deliver the current object from {source_device_ref} to {destination_device_ref}.")
                    }),
                    capability_requirements: vec![CapabilityRequirement {
                        category: GeneratedUserVisibleText::from_semantic("file_transfer"),
                        user_visible_requirement: GeneratedUserVisibleText::from_semantic(if pipeline { "Use encrypted PipelinePrivate landing for this explicit intermediate Transfer." } else { "Deliver only the object produced by the preceding approved step." }),
                    }],
                    failure_behavior: StepFailureBehavior::StopPlan,
                    source: ObjectSelectionRule::FromSlot { slot_id: input_slot },
                    destination,
                });
                if pipeline {
                    current_slot = Some((output_slot_id, input_contract));
                    current_device = Some(destination_device_ref.clone());
                } else {
                    current_slot = None;
                    current_device = None;
                }
                explanations.push(StepExplanation {
                    step_id: step_id.clone(),
                    action_summary: GeneratedUserVisibleText::from_semantic(if pipeline { format!("Private pipeline Transfer from {source_device_ref} to {destination_device_ref}.") } else { format!("Final Transfer from {source_device_ref} to {destination_device_ref}.") }),
                    expected_result: GeneratedUserVisibleText::from_semantic(if pipeline { "A one-use private object is local to the following approved step." } else { "The approved final destination receives the object." }),
                });
            }
            ComposedFilePlanBlock::Execute {
                execution_device_ref,
                target_revision,
                execution_intent,
            } => {
                if !matches_bridge(&execution_device_ref) {
                    return invalid_revision(
                        "The composed Execute execution device is outside this Bridge.",
                    );
                }
                let (input_slot, input_contract) = current_slot.clone().ok_or_else(|| {
                    AppError::InvalidInput("Execute needs a local input object.".into())
                })?;
                if current_device.as_deref() != Some(execution_device_ref.as_str()) {
                    return invalid_revision("Execute input is not local to its approved execution device; add an explicit Transfer first.");
                }
                if current_revision != Some(target_revision.revision)
                    || target_revision.logical_object_id != "selected_file"
                {
                    return invalid_revision(
                        "Execute must consume the current logical object revision.",
                    );
                }
                let execution_intent = RawUserGoal::from(execution_intent);
                execution_intent.validate("Execute execution intent")?;
                steps.push(BridgePlanStep::Execute {
                    step_id: step_id.clone(),
                    depends_on,
                    input_slots: vec![PlanSlot {
                        slot_id: input_slot.clone(),
                        object: input_contract,
                        cardinality: SlotCardinality::One,
                    }],
                    output_slots: Vec::new(),
                    source_device_ref: Some(execution_device_ref.clone()),
                    execution_device_ref: execution_device_ref.clone(),
                    user_visible_action: GeneratedUserVisibleText::from_semantic(format!(
                        "Authorize the reviewed execution intent on {execution_device_ref}."
                    )),
                    capability_requirements: Vec::new(),
                    failure_behavior: StepFailureBehavior::RequireNewRevision,
                    target: ObjectSelectionRule::FromSlot {
                        slot_id: input_slot,
                    },
                    target_revision,
                    execution_intent,
                });
                explanations.push(StepExplanation {
                    step_id: step_id.clone(),
                    action_summary: GeneratedUserVisibleText::from_semantic(format!(
                        "Authorize execution intent on {execution_device_ref}."
                    )),
                    expected_result: GeneratedUserVisibleText::from_semantic(
                        "The reviewed execution intent remains part of the immutable Plan, but is not currently executable without a future Agent implementation.",
                    ),
                });
            }
        }
        previous_step = Some(step_id);
    }

    let search_selection_mode = if steps.iter().any(|step| {
        matches!(
            step,
            BridgePlanStep::Search {
                selection: Some(_),
                ..
            }
        )
    }) {
        SearchSelectionMode::BoundedInline
    } else {
        SearchSelectionMode::Staged
    };
    let flow = steps
        .iter()
        .map(|step| match step.operation() {
            StepOperation::Search => "Search",
            StepOperation::Transform => "Transform",
            StepOperation::Transfer => "Transfer",
            StepOperation::Execute => "Execute",
        })
        .collect::<Vec<_>>()
        .join(" → ");
    let mut revision = BridgePlanRevision {
        schema_version: GeneratedUserVisibleText::from_semantic("bridge-plan-v1"),
        plan_id: format!("plan-{}", uuid::Uuid::new_v4()),
        revision_id: format!("revision-{}", uuid::Uuid::new_v4()),
        revision_number: 1,
        revision_hash: String::new(),
        bridge_id,
        requesting_device_ref,
        selected_device_ref,
        original_user_goal: RawUserGoal::from(original_user_goal),
        presentation: BridgePlanPresentation {
            title: GeneratedUserVisibleText::from_semantic(format!("{flow} file plan")),
            natural_language_plan: GeneratedUserVisibleText::from_semantic(format!(
                "Execute the explicitly reviewed {flow} object flow on its stated devices."
            )),
            step_explanations: explanations,
        },
        expected_outcome: GeneratedUserVisibleText::from_semantic(
            "The object remains and moves only as stated by the approved steps.",
        ),
        search_selection_mode,
        steps,
        alternative: None,
    };
    validate_revision(&revision)?;
    revision.revision_hash = canonical_revision_hash(&revision)?;
    Ok(revision)
}

/// Pastey Core can review these primitives, but it must not create attempt
/// authority until a future Agent implementation can faithfully execute them.
pub(crate) fn framework_execution_unavailable(revision: &BridgePlanRevision) -> bool {
    revision.steps.iter().any(|step| {
        matches!(
            step,
            BridgePlanStep::Transform { .. } | BridgePlanStep::Execute { .. }
        )
    })
}

/// Builds the supported file-based Bridge Plan shapes from bounded product
/// intent. The Host, rather than the renderer or provider, fixes device
/// bindings, object slots, selection rules, and executable semantics.
#[cfg(test)]
pub(crate) fn build_file_plan_revision(
    bridge_id: String,
    requesting_device_ref: String,
    selected_device_ref: String,
    original_user_goal: String,
    filename_hint: String,
    extensions: Vec<String>,
    safe_scope_labels: Vec<String>,
    transfer_to_requester: bool,
) -> AppResult<BridgePlanRevision> {
    const ALLOWED_SCOPES: &[&str] = &["downloads", "desktop", "documents", "pastey_shared"];
    if safe_scope_labels.is_empty()
        || safe_scope_labels
            .iter()
            .any(|scope| !ALLOWED_SCOPES.contains(&scope.as_str()))
    {
        return Err(AppError::InvalidInput(
            "Bridge Plan Search must use one or more supported safe locations.".into(),
        ));
    }
    let extensions = extensions
        .into_iter()
        .map(|extension| GeneratedUserVisibleText::from_semantic(extension.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    let query = filename_hint.trim().to_owned();
    if query.is_empty() {
        return Err(AppError::InvalidInput(
            "Bridge Plan Search needs a filename or file description.".into(),
        ));
    }
    let object = ObjectContract {
        object_type: GeneratedUserVisibleText::from_semantic("file"),
        media_types: Vec::new(),
        user_visible_description: GeneratedUserVisibleText::from_semantic("a matching file"),
    };
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let revision_id = format!("revision-{}", uuid::Uuid::new_v4());
    let search_step_id = "search".to_owned();
    let transfer_step_id = "transfer".to_owned();
    let selected_slot_id = "selected_file".to_owned();
    let search_selection = transfer_to_requester.then(|| BoundedSearchSelectionRule {
        source_slot_id: "found".into(),
        result_set_limit: 10,
        allowed_object: object.clone(),
        downstream_slot_id: selected_slot_id.clone(),
    });
    let mut steps = vec![BridgePlanStep::Search {
        step_id: search_step_id.clone(),
        depends_on: Vec::new(),
        input_slots: Vec::new(),
        output_slots: vec![PlanSlot {
            slot_id: "found".into(),
            object: object.clone(),
            cardinality: SlotCardinality::Many,
        }],
        source_device_ref: Some(selected_device_ref.clone()),
        execution_device_ref: selected_device_ref.clone(),
        user_visible_action: GeneratedUserVisibleText::from_semantic(
            "Search the selected device for matching files.",
        ),
        capability_requirements: vec![CapabilityRequirement {
            category: GeneratedUserVisibleText::from_semantic("object_search"),
            user_visible_requirement: GeneratedUserVisibleText::from_semantic(
                "Search only the reviewed locations.",
            ),
        }],
        failure_behavior: StepFailureBehavior::StopPlan,
        query: SearchIntent {
            query: GeneratedUserVisibleText::from_semantic(query),
            extensions,
            safe_scope_labels: safe_scope_labels
                .into_iter()
                .map(SafeLocationDescription::from)
                .collect(),
        },
        selection: search_selection,
    }];
    let mut step_explanations = vec![StepExplanation {
        step_id: search_step_id,
        action_summary: GeneratedUserVisibleText::from_semantic(
            "Search reviewed locations for matching files.",
        ),
        expected_result: GeneratedUserVisibleText::from_semantic(if transfer_to_requester {
            "A bounded list of matches so one file can be selected for transfer."
        } else {
            "A bounded summary of matching files."
        }),
    }];
    if transfer_to_requester {
        steps.push(BridgePlanStep::Transfer {
            step_id: transfer_step_id.clone(),
            depends_on: vec!["search".into()],
            input_slots: vec![PlanSlot {
                slot_id: selected_slot_id.clone(),
                object: object.clone(),
                cardinality: SlotCardinality::One,
            }],
            output_slots: Vec::new(),
            source_device_ref: Some(selected_device_ref.clone()),
            execution_device_ref: selected_device_ref.clone(),
            user_visible_action: GeneratedUserVisibleText::from_semantic(
                "Transfer the selected matching file to the requesting device.",
            ),
            capability_requirements: vec![CapabilityRequirement {
                category: GeneratedUserVisibleText::from_semantic("file_transfer"),
                user_visible_requirement: GeneratedUserVisibleText::from_semantic(
                    "Send only the file selected from this plan's bounded Search results.",
                ),
            }],
            failure_behavior: StepFailureBehavior::StopPlan,
            source: ObjectSelectionRule::FromSlot {
                slot_id: selected_slot_id,
            },
            destination: TransferDestination::RequestingDevice {
                device_ref: requesting_device_ref.clone(),
            },
        });
        step_explanations.push(StepExplanation {
            step_id: transfer_step_id,
            action_summary: GeneratedUserVisibleText::from_semantic(
                "Transfer the selected file to the requesting device.",
            ),
            expected_result: GeneratedUserVisibleText::from_semantic(
                "The selected file is delivered through the Bridge transfer channel.",
            ),
        });
    }
    let mut revision = BridgePlanRevision {
        schema_version: GeneratedUserVisibleText::from_semantic("bridge-plan-v1"),
        plan_id,
        revision_id,
        revision_number: 1,
        revision_hash: String::new(),
        bridge_id,
        requesting_device_ref,
        selected_device_ref: selected_device_ref.clone(),
        original_user_goal: RawUserGoal::from(original_user_goal),
        presentation: BridgePlanPresentation {
            title: GeneratedUserVisibleText::from_semantic(if transfer_to_requester {
                "Search and transfer a file from the selected device"
            } else {
                "Search files on selected device"
            }),
            natural_language_plan: GeneratedUserVisibleText::from_semantic(
                if transfer_to_requester {
                    "Search the selected device's reviewed locations for matching files. After the requester selects one bounded result, transfer that file to the requesting device."
                } else {
                    "Search the selected device's reviewed locations for matching files and return a bounded summary."
                },
            ),
            step_explanations,
        },
        expected_outcome: GeneratedUserVisibleText::from_semantic(if transfer_to_requester {
            "One requester-selected matching file is transferred to the requesting device."
        } else {
            "A bounded Search summary is returned to the requesting device."
        }),
        search_selection_mode: if transfer_to_requester {
            SearchSelectionMode::BoundedInline
        } else {
            SearchSelectionMode::Staged
        },
        steps,
        alternative: None,
    };
    validate_revision(&revision)?;
    revision.revision_hash = canonical_revision_hash(&revision)?;
    Ok(revision)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RevisionRecord {
    pub(crate) revision: BridgePlanRevision,
    pub(crate) state: RevisionState,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanApproval {
    pub(crate) approval_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) bridge_id: String,
    pub(crate) requester_device_ref: String,
    pub(crate) selected_device_ref: String,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ApprovalRecord {
    pub(crate) approval: BridgePlanApproval,
    pub(crate) state: ApprovalState,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanAttempt {
    pub(crate) attempt_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) revision_hash: String,
    pub(crate) approval_id: String,
    pub(crate) bridge_id: String,
    pub(crate) graph_projection: SafeGraphProjection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SafeGraphProjection {
    pub(crate) graph_id: String,
    pub(crate) derived_from_revision_hash: String,
    pub(crate) graph_hash: String,
    pub(crate) nodes: Vec<SafeGraphNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SafeGraphNode {
    pub(crate) node_id: String,
    pub(crate) step_id: String,
    pub(crate) operation: StepOperation,
    pub(crate) depends_on_node_ids: Vec<String>,
    pub(crate) input_slots: Vec<PlanSlot>,
    pub(crate) output_slots: Vec<PlanSlot>,
    pub(crate) source_device_ref: Option<String>,
    pub(crate) execution_device_ref: String,
    /// Exact semantic step copied from the immutable revision. This is a
    /// durable, platform-neutral projection, never an execution command.
    pub(crate) step: BridgePlanStep,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepExecutionState {
    Pending,
    Eligible,
    Authorized,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl StepExecutionState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Eligible => "eligible",
            Self::Authorized => "authorized",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StepExecutionProjection {
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) state: StepExecutionState,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AttemptRecord {
    pub(crate) attempt: BridgePlanAttempt,
    pub(crate) state: AttemptState,
    pub(crate) created_at: i64,
    pub(crate) started_at: Option<i64>,
    pub(crate) ended_at: Option<i64>,
    pub(crate) interruption_reason: Option<SafeActivitySummary>,
    pub(crate) steps: Vec<StepExecutionProjection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanActivity {
    pub(crate) activity_id: String,
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) attempt_id: Option<String>,
    pub(crate) step_id: Option<String>,
    pub(crate) kind: ActivityKind,
    pub(crate) occurred_at: i64,
    pub(crate) summary: SafeActivitySummary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BridgePlanResultSummary {
    pub(crate) result_id: String,
    pub(crate) bridge_id: String,
    pub(crate) plan_id: String,
    pub(crate) revision_id: String,
    pub(crate) attempt_id: String,
    pub(crate) step_id: String,
    pub(crate) status: GeneratedUserVisibleText,
    pub(crate) summary: SafeActivitySummary,
    pub(crate) produced_object_description: Option<GeneratedUserVisibleText>,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct BridgePlanRecords {
    pub(crate) plans: Vec<BridgePlan>,
    pub(crate) revisions: Vec<RevisionRecord>,
    pub(crate) approvals: Vec<ApprovalRecord>,
    pub(crate) attempts: Vec<AttemptRecord>,
    pub(crate) activities: Vec<BridgePlanActivity>,
    pub(crate) results: Vec<BridgePlanResultSummary>,
}

pub(crate) fn init_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS bridge_plans (
            plan_id TEXT PRIMARY KEY, bridge_id TEXT NOT NULL,
            requesting_device_ref TEXT NOT NULL, created_at INTEGER NOT NULL, state TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_revisions (
            revision_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, bridge_id TEXT NOT NULL,
            revision_number INTEGER NOT NULL, revision_hash TEXT NOT NULL, created_at INTEGER NOT NULL,
            state TEXT NOT NULL, revision_json TEXT NOT NULL,
            UNIQUE(plan_id, revision_number), UNIQUE(plan_id, revision_hash),
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_approvals (
            approval_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, revision_id TEXT NOT NULL,
            bridge_id TEXT NOT NULL, created_at INTEGER NOT NULL, state TEXT NOT NULL,
            approval_json TEXT NOT NULL,
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_revisions(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_attempts (
            attempt_id TEXT PRIMARY KEY, approval_id TEXT NOT NULL UNIQUE, plan_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, bridge_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            state TEXT NOT NULL, started_at INTEGER, ended_at INTEGER, interruption_reason TEXT,
            attempt_json TEXT NOT NULL,
            FOREIGN KEY(approval_id) REFERENCES bridge_plan_approvals(approval_id) ON DELETE CASCADE,
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_revisions(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_attempt_steps (
            attempt_id TEXT NOT NULL,
            step_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('pending','eligible','authorized','running','completed','failed','cancelled')),
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(attempt_id, step_id),
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_activities (
            activity_id TEXT PRIMARY KEY, bridge_id TEXT NOT NULL, plan_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, attempt_id TEXT, occurred_at INTEGER NOT NULL,
            activity_json TEXT NOT NULL,
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_revisions(revision_id) ON DELETE CASCADE,
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_results (
            result_id TEXT PRIMARY KEY, bridge_id TEXT NOT NULL, plan_id TEXT NOT NULL,
            revision_id TEXT NOT NULL, attempt_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            result_json TEXT NOT NULL,
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_revisions(revision_id) ON DELETE CASCADE,
            FOREIGN KEY(attempt_id) REFERENCES bridge_plan_attempts(attempt_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_bridge_plans_bridge ON bridge_plans(bridge_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_bridge_plan_revisions_bridge ON bridge_plan_revisions(bridge_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_bridge_plan_attempts_bridge ON bridge_plan_attempts(bridge_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_bridge_plan_attempt_steps_attempt ON bridge_plan_attempt_steps(attempt_id, state);
        CREATE INDEX IF NOT EXISTS idx_bridge_plan_activities_bridge ON bridge_plan_activities(bridge_id, occurred_at);

        CREATE TRIGGER IF NOT EXISTS bridge_plan_identity_immutable
        BEFORE UPDATE OF bridge_id, requesting_device_ref, created_at ON bridge_plans
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan identity is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_immutable
        BEFORE UPDATE OF plan_id, bridge_id, revision_number, revision_hash, created_at, revision_json
        ON bridge_plan_revisions
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan revision is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_immutable
        BEFORE UPDATE OF plan_id, revision_id, bridge_id, created_at, approval_json
        ON bridge_plan_approvals
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan approval is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_state_guard
        BEFORE UPDATE OF state ON bridge_plan_approvals
        WHEN NOT (
            (OLD.state = 'awaiting_receiver' AND NEW.state IN ('valid', 'denied', 'expired', 'revoked', 'burned')) OR
            (OLD.state = 'valid' AND NEW.state IN ('consumed', 'expired', 'revoked', 'burned')) OR
            (OLD.state IN ('denied', 'expired', 'revoked', 'consumed') AND NEW.state = 'burned')
        )
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan approval transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_immutable
        BEFORE UPDATE OF approval_id, plan_id, revision_id, bridge_id, created_at, attempt_json
        ON bridge_plan_attempts
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan attempt is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_step_transition_guard
        BEFORE UPDATE OF state ON bridge_plan_attempt_steps
        WHEN NOT (
            (OLD.state = 'pending' AND NEW.state = 'eligible') OR
            (OLD.state = 'eligible' AND NEW.state IN ('authorized', 'cancelled')) OR
            (OLD.state = 'authorized' AND NEW.state IN ('running', 'cancelled')) OR
            (OLD.state = 'running' AND NEW.state IN ('completed', 'failed', 'cancelled'))
        )
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan step transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_step_after_burn
        BEFORE UPDATE ON bridge_plan_attempt_steps
        WHEN EXISTS(
            SELECT 1 FROM bridge_plan_attempts
            JOIN burned_bridges ON burned_bridges.room_id = bridge_plan_attempts.bridge_id
            WHERE bridge_plan_attempts.attempt_id = OLD.attempt_id
        )
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_step_insert_after_burn
        BEFORE INSERT ON bridge_plan_attempt_steps
        WHEN EXISTS(
            SELECT 1 FROM bridge_plan_attempts
            JOIN burned_bridges ON burned_bridges.room_id = bridge_plan_attempts.bridge_id
            WHERE bridge_plan_attempts.attempt_id = NEW.attempt_id
        )
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_reinsert_guard
        BEFORE INSERT ON bridge_plan_revisions
        WHEN EXISTS (SELECT 1 FROM bridge_plan_revisions WHERE revision_id = NEW.revision_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan revisions cannot be replaced'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_mutation_after_burn
        BEFORE INSERT ON bridge_plans
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_mutation_after_burn
        BEFORE INSERT ON bridge_plan_revisions
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_mutation_after_burn
        BEFORE INSERT ON bridge_plan_approvals
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_mutation_after_burn
        BEFORE INSERT ON bridge_plan_attempts
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_activity_mutation_after_burn
        BEFORE INSERT ON bridge_plan_activities
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_result_mutation_after_burn
        BEFORE INSERT ON bridge_plan_results
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_state_after_burn
        BEFORE UPDATE ON bridge_plans
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_state_after_burn
        BEFORE UPDATE ON bridge_plan_revisions
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_state_after_burn
        BEFORE UPDATE ON bridge_plan_approvals
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_state_after_burn
        BEFORE UPDATE ON bridge_plan_attempts
        WHEN EXISTS (SELECT 1 FROM burned_bridges WHERE room_id = NEW.bridge_id)
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan cannot change after Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_count_cap
        BEFORE INSERT ON bridge_plans
        WHEN (SELECT COUNT(*) FROM bridge_plans WHERE bridge_id = NEW.bridge_id) >= 128
        BEGIN SELECT RAISE(ABORT, 'too many plans for this Bridge'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_count_cap
        BEFORE INSERT ON bridge_plan_revisions
        WHEN (SELECT COUNT(*) FROM bridge_plan_revisions WHERE plan_id = NEW.plan_id) >= 64
        BEGIN SELECT RAISE(ABORT, 'too many revisions for this plan'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_count_cap
        BEFORE INSERT ON bridge_plan_approvals
        WHEN (SELECT COUNT(*) FROM bridge_plan_approvals WHERE revision_id = NEW.revision_id) >= 128
        BEGIN SELECT RAISE(ABORT, 'too many approvals for this revision'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_count_cap
        BEFORE INSERT ON bridge_plan_attempts
        WHEN (SELECT COUNT(*) FROM bridge_plan_attempts WHERE revision_id = NEW.revision_id) >= 256
        BEGIN SELECT RAISE(ABORT, 'too many attempts for this revision'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_live_attempt_count_cap
        BEFORE INSERT ON bridge_plan_attempts
        WHEN (SELECT COUNT(*) FROM bridge_plan_attempts WHERE plan_id = NEW.plan_id AND state IN ('created', 'running')) >= 1024
        BEGIN SELECT RAISE(ABORT, 'too many live attempts for this plan'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_activity_count_cap
        BEFORE INSERT ON bridge_plan_activities
        WHEN (SELECT COUNT(*) FROM bridge_plan_activities WHERE plan_id = NEW.plan_id) >= 1024
        BEGIN SELECT RAISE(ABORT, 'too many activities for this plan'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_result_count_cap
        BEFORE INSERT ON bridge_plan_results
        WHEN (SELECT COUNT(*) FROM bridge_plan_results WHERE attempt_id = NEW.attempt_id) >= 128
        BEGIN SELECT RAISE(ABORT, 'too many results for this attempt'); END;
    "#)?;
    // Existing stores may still contain inert columns/tables from the removed
    // receiver-per-plan approval model. Drop its executable guards; the live
    // approval path is requester-owned and always starts in `valid`.
    for trigger in [
        "bridge_plan_receiver_evidence_immutable",
        "bridge_plan_receiver_decision_guard",
        "bridge_plan_receiver_decision_insert_guard",
        "bridge_plan_receiver_decision_update_immutable",
    ] {
        conn.execute(&format!("DROP TRIGGER IF EXISTS {trigger}"), [])?;
    }
    // Remove the former writable marker escape hatch before installing the
    // permanent guards.  Only the private Burn repository temporarily lifts
    // these guards within its own transaction.
    drop_delete_guards(conn)?;
    conn.execute("DROP TABLE IF EXISTS bridge_plan_burn_deletions", [])?;
    create_delete_guards(conn)?;
    protocol::init_schema(conn)?;
    Ok(())
}

const DELETE_GUARD_TRIGGERS: &[&str] = &[
    "bridge_plan_revision_delete_guard",
    "bridge_plan_delete_guard",
    "bridge_plan_approval_delete_guard",
    "bridge_plan_attempt_delete_guard",
    "bridge_plan_attempt_step_delete_guard",
    "bridge_plan_activity_delete_guard",
    "bridge_plan_result_delete_guard",
    "bridge_plan_receiver_decision_delete_guard",
];

fn create_delete_guards(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TRIGGER IF NOT EXISTS bridge_plan_revision_delete_guard
        BEFORE DELETE ON bridge_plan_revisions
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan revisions are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_delete_guard
        BEFORE DELETE ON bridge_plans
        BEGIN SELECT RAISE(ABORT, 'Bridge Plans are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_delete_guard
        BEFORE DELETE ON bridge_plan_approvals
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan approvals are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_delete_guard
        BEFORE DELETE ON bridge_plan_attempts
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan attempts are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_attempt_step_delete_guard
        BEFORE DELETE ON bridge_plan_attempt_steps
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan step projections are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_activity_delete_guard
        BEFORE DELETE ON bridge_plan_activities
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan activities are removed only by scoped Burn'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_result_delete_guard
        BEFORE DELETE ON bridge_plan_results
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan results are removed only by scoped Burn'); END;
        "#,
    )?;
    Ok(())
}

fn drop_delete_guards(conn: &Connection) -> AppResult<()> {
    for trigger in DELETE_GUARD_TRIGGERS {
        conn.execute(&format!("DROP TRIGGER IF EXISTS {trigger}"), [])?;
    }
    Ok(())
}

pub(crate) fn canonical_revision_hash(revision: &BridgePlanRevision) -> AppResult<String> {
    validate_revision(revision)?;
    let payload = SemanticRevision::from(revision);
    let canonical = canonical_json(&serde_json::to_value(payload)?);
    Ok(format!(
        "{HASH_VERSION}:{}",
        blake3::hash(format!("{HASH_VERSION}\0{canonical}").as_bytes()).to_hex()
    ))
}

/// Deterministically lowers one immutable revision into a platform-neutral
/// execution graph. It copies only revision semantics; it never selects a
/// worker, command, path, sandbox, or other backend detail.
pub(crate) fn compile_graph_projection(
    revision: &BridgePlanRevision,
) -> AppResult<SafeGraphProjection> {
    validate_revision(revision)?;
    if canonical_revision_hash(revision)? != revision.revision_hash {
        return Err(AppError::InvalidInput(
            "Bridge Plan revision hash does not match its semantic payload.".into(),
        ));
    }
    let nodes = revision
        .steps
        .iter()
        .map(|step| SafeGraphNode {
            node_id: format!("revision-step:{}", step.id()),
            step_id: step.id().into(),
            operation: step.operation(),
            depends_on_node_ids: step
                .dependencies()
                .iter()
                .map(|dependency| format!("revision-step:{dependency}"))
                .collect(),
            input_slots: step.inputs().to_vec(),
            output_slots: step.outputs().to_vec(),
            source_device_ref: step.source_device().map(str::to_owned),
            execution_device_ref: step.execution_device().into(),
            step: step.clone(),
        })
        .collect::<Vec<_>>();
    let mut graph = SafeGraphProjection {
        graph_id: format!("revision-graph:{}", revision.revision_id),
        derived_from_revision_hash: revision.revision_hash.clone(),
        graph_hash: String::new(),
        nodes,
    };
    graph.graph_hash = canonical_graph_hash(&graph)?;
    validate_graph_projection(&graph, revision)?;
    Ok(graph)
}

fn canonical_graph_hash(graph: &SafeGraphProjection) -> AppResult<String> {
    let value = serde_json::json!({
        "derived_from_revision_hash": graph.derived_from_revision_hash,
        "nodes": graph.nodes,
    });
    Ok(format!(
        "bridge-plan-graph-v1:{}",
        blake3::hash(canonical_json(&value).as_bytes()).to_hex()
    ))
}

#[derive(Serialize)]
struct SemanticRevision<'a> {
    hash_version: &'static str,
    schema_version: &'a GeneratedUserVisibleText,
    bridge_id: &'a str,
    requesting_device_ref: &'a str,
    selected_device_ref: &'a str,
    original_user_goal: &'a RawUserGoal,
    presentation: &'a BridgePlanPresentation,
    expected_outcome: &'a GeneratedUserVisibleText,
    search_selection_mode: &'a SearchSelectionMode,
    steps: Vec<BridgePlanStep>,
    alternative: &'a Option<AlternativeProposal>,
}
impl<'a> From<&'a BridgePlanRevision> for SemanticRevision<'a> {
    fn from(value: &'a BridgePlanRevision) -> Self {
        Self {
            hash_version: HASH_VERSION,
            schema_version: &value.schema_version,
            bridge_id: &value.bridge_id,
            requesting_device_ref: &value.requesting_device_ref,
            selected_device_ref: &value.selected_device_ref,
            original_user_goal: &value.original_user_goal,
            presentation: &value.presentation,
            expected_outcome: &value.expected_outcome,
            search_selection_mode: &value.search_selection_mode,
            steps: canonical_steps(&value.steps),
            alternative: &value.alternative,
        }
    }
}

fn canonical_steps(steps: &[BridgePlanStep]) -> Vec<BridgePlanStep> {
    let mut result = steps.to_vec();
    for step in &mut result {
        canonicalize_step(step);
    }
    result
}
fn canonicalize_step(step: &mut BridgePlanStep) {
    let (dependencies, inputs, outputs, requirements) = match step {
        BridgePlanStep::Search {
            depends_on,
            input_slots,
            output_slots,
            capability_requirements,
            ..
        }
        | BridgePlanStep::Transform {
            depends_on,
            input_slots,
            output_slots,
            capability_requirements,
            ..
        }
        | BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            output_slots,
            capability_requirements,
            ..
        }
        | BridgePlanStep::Execute {
            depends_on,
            input_slots,
            output_slots,
            capability_requirements,
            ..
        } => (
            depends_on,
            input_slots,
            output_slots,
            capability_requirements,
        ),
    };
    dependencies.sort();
    inputs.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));
    outputs.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));
    for slot in inputs.iter_mut().chain(outputs.iter_mut()) {
        slot.object.media_types.sort();
    }
    requirements.sort_by(|a, b| {
        (a.category.as_str(), a.user_visible_requirement.as_str())
            .cmp(&(b.category.as_str(), b.user_visible_requirement.as_str()))
    });
    match step {
        BridgePlanStep::Search {
            query, selection, ..
        } => {
            query.safe_scope_labels.sort();
            if let Some(selection) = selection {
                selection.allowed_object.media_types.sort();
            }
        }
        BridgePlanStep::Transform { expected_input, .. } => {
            expected_input.media_types.sort();
        }
        BridgePlanStep::Transfer {
            source: ObjectSelectionRule::FutureUserSelection { object, .. },
            ..
        } => object.media_types.sort(),
        _ => {}
    }
}
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string serializes"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let values = values.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                values
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key serializes"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn step_output_device(step: &BridgePlanStep) -> AppResult<&str> {
    match step {
        BridgePlanStep::Transfer { destination, .. } => match destination {
            TransferDestination::PipelineHandoff { device_ref }
            | TransferDestination::RequestingDevice { device_ref }
            | TransferDestination::SelectedDevice { device_ref }
            | TransferDestination::UserSelectedLocation { device_ref, .. } => Ok(device_ref),
        },
        BridgePlanStep::Search { .. }
        | BridgePlanStep::Transform { .. }
        | BridgePlanStep::Execute { .. } => Ok(step.execution_device()),
    }
}

pub(crate) fn validate_revision(revision: &BridgePlanRevision) -> AppResult<()> {
    id(&revision.plan_id, "plan id")?;
    id(&revision.revision_id, "revision id")?;
    id(&revision.bridge_id, "bridge id")?;
    id(&revision.requesting_device_ref, "requesting device")?;
    id(&revision.selected_device_ref, "selected device")?;
    if revision.requesting_device_ref == revision.selected_device_ref {
        return invalid("Bridge Plan v1 requires two distinct Bridge devices.");
    }
    revision.schema_version.validate("schema version")?;
    revision.original_user_goal.validate("original user goal")?;
    revision.expected_outcome.validate("expected outcome")?;
    revision.presentation.title.validate("presentation title")?;
    revision
        .presentation
        .natural_language_plan
        .validate("natural-language plan")?;
    if revision.steps.is_empty() || revision.steps.len() > MAX_STEPS {
        return invalid("Bridge Plan revision has an invalid number of steps.");
    }
    let mut steps = HashSet::new();
    let mut output_owner = HashMap::new();
    let mut output_location: HashMap<&str, String> = HashMap::new();
    let mut logical_revisions: HashMap<String, LogicalObjectRevision> = HashMap::new();
    for step in &revision.steps {
        id(step.id(), "step id")?;
        if !steps.insert(step.id()) {
            return invalid("Bridge Plan revision has duplicate step IDs.");
        }
        if !matches_device(step.execution_device(), revision)
            || step
                .source_device()
                .is_some_and(|device| !matches_device(device, revision))
        {
            return invalid("Bridge Plan v1 step references a device outside its Bridge.");
        }
        validate_step_text(step)?;
        if step.inputs().len() > MAX_SLOTS_PER_STEP || step.outputs().len() > MAX_SLOTS_PER_STEP {
            return invalid("Bridge Plan step has too many slots.");
        }
        if has_duplicate_ids(step.inputs().iter().map(|slot| slot.slot_id.as_str()))
            || has_duplicate_ids(step.outputs().iter().map(|slot| slot.slot_id.as_str()))
        {
            return invalid("Bridge Plan step has duplicate slots.");
        }
        for slot in step.outputs() {
            id(&slot.slot_id, "output slot")?;
            validate_contract(&slot.object)?;
            if output_owner
                .insert(slot.slot_id.as_str(), step.id())
                .is_some()
            {
                return invalid("Bridge Plan slot has more than one producer.");
            }
            output_location.insert(slot.slot_id.as_str(), step_output_device(step)?.to_owned());
        }
        for slot in step.inputs() {
            id(&slot.slot_id, "input slot")?;
            validate_contract(&slot.object)?;
        }
        if step.dependencies().len() > MAX_DEPENDENCIES {
            return invalid("Bridge Plan step has too many dependencies.");
        }
        if step.dependencies().len() != step.dependencies().iter().collect::<HashSet<_>>().len() {
            return invalid("Bridge Plan step has duplicate dependencies.");
        }
        for dependency in step.dependencies() {
            id(dependency, "step dependency")?;
        }
        if let BridgePlanStep::Transfer {
            destination,
            source,
            ..
        } = step
        {
            validate_destination(destination, revision)?;
            if let ObjectSelectionRule::FutureUserSelection {
                object,
                selection_prompt,
            } = source
            {
                validate_contract(object)?;
                selection_prompt.validate("selection prompt")?;
            }
        }
        if let BridgePlanStep::Search {
            selection: Some(selection),
            output_slots,
            ..
        } = step
        {
            if revision.search_selection_mode != SearchSelectionMode::BoundedInline
                || selection.result_set_limit == 0
                || selection.result_set_limit > 128
            {
                return invalid("Bridge Plan bounded Search selection is invalid.");
            }
            id(&selection.source_slot_id, "Search selection source slot")?;
            id(
                &selection.downstream_slot_id,
                "Search selection downstream slot",
            )?;
            if selection.source_slot_id == selection.downstream_slot_id
                || !output_slots
                    .iter()
                    .any(|slot| slot.slot_id == selection.source_slot_id)
            {
                return invalid(
                    "Bridge Plan Search selection must derive from a Search output slot.",
                );
            }
            if output_owner
                .insert(selection.downstream_slot_id.as_str(), step.id())
                .is_some()
            {
                return invalid("Bridge Plan selected-result slot has more than one producer.");
            }
            output_location.insert(
                selection.downstream_slot_id.as_str(),
                step.execution_device().to_owned(),
            );
            validate_contract(&selection.allowed_object)?;
            logical_revisions.insert(
                selection.downstream_slot_id.clone(),
                LogicalObjectRevision {
                    logical_object_id: "selected_file".into(),
                    revision: 1,
                },
            );
        }
        if let BridgePlanStep::Search {
            selection: None, ..
        } = step
        {
            if revision.search_selection_mode == SearchSelectionMode::BoundedInline {
                return invalid(
                    "Bridge Plan bounded inline selection requires a Search selection rule.",
                );
            }
        }
    }
    let explanations = revision
        .presentation
        .step_explanations
        .iter()
        .map(|entry| {
            id(&entry.step_id, "presentation step id")?;
            entry.action_summary.validate("step action summary")?;
            entry.expected_result.validate("step expected result")?;
            Ok(entry.step_id.as_str())
        })
        .collect::<AppResult<HashSet<_>>>()?;
    if explanations.len() != revision.steps.len() || explanations != steps {
        return invalid("Bridge Plan presentation must map one-to-one to revision steps.");
    }
    for step in &revision.steps {
        for dependency in step.dependencies() {
            if !steps.contains(dependency.as_str()) || dependency == step.id() {
                return invalid("Bridge Plan step dependency is invalid.");
            }
        }
        for slot in step.inputs() {
            let Some(owner) = output_owner.get(slot.slot_id.as_str()) else {
                return invalid("Bridge Plan input slot has no producer.");
            };
            if !step
                .dependencies()
                .iter()
                .any(|dependency| dependency == *owner)
            {
                return invalid("Bridge Plan input slot producer is not a dependency.");
            }
            let location = output_location.get(slot.slot_id.as_str()).ok_or_else(|| {
                AppError::InvalidInput("Bridge Plan input object location is unavailable.".into())
            })?;
            if matches!(
                step,
                BridgePlanStep::Transform { .. }
                    | BridgePlanStep::Transfer { .. }
                    | BridgePlanStep::Execute { .. }
            ) && step.source_device() != Some(location.as_str())
            {
                return invalid("Bridge Plan step consumes an object at the wrong device.");
            }
        }
        if let BridgePlanStep::Transfer {
            source: ObjectSelectionRule::FromSlot { slot_id },
            ..
        } = step
        {
            if !step.inputs().iter().any(|input| input.slot_id == *slot_id) {
                return invalid("Bridge Plan Transfer source slot is not an input slot.");
            }
        }
        if let BridgePlanStep::Transform {
            target: ObjectSelectionRule::FromSlot { slot_id },
            input_revision,
            output_revision,
            output_slots,
            ..
        } = step
        {
            if !step.inputs().iter().any(|input| input.slot_id == *slot_id) {
                return invalid("Bridge Plan action target slot is not an input slot.");
            }
            if logical_revisions.get(slot_id) != Some(input_revision) {
                return invalid(
                    "Bridge Plan Transform consumes the wrong logical object revision.",
                );
            }
            for output in output_slots {
                logical_revisions.insert(output.slot_id.clone(), output_revision.clone());
            }
        }
        if let BridgePlanStep::Execute {
            target: ObjectSelectionRule::FromSlot { slot_id },
            target_revision,
            ..
        } = step
        {
            if !step.inputs().iter().any(|input| input.slot_id == *slot_id) {
                return invalid("Bridge Plan action target slot is not an input slot.");
            }
            if logical_revisions.get(slot_id) != Some(target_revision) {
                return invalid("Bridge Plan Execute consumes the wrong logical object revision.");
            }
        }
        if let BridgePlanStep::Transfer {
            source: ObjectSelectionRule::FromSlot { slot_id },
            output_slots,
            ..
        } = step
        {
            if let Some(revision) = logical_revisions.get(slot_id).cloned() {
                for output in output_slots {
                    logical_revisions.insert(output.slot_id.clone(), revision.clone());
                }
            }
        }
    }
    validate_acyclic(&revision.steps)
}

fn validate_step_text(step: &BridgePlanStep) -> AppResult<()> {
    let (action, requirements) = match step {
        BridgePlanStep::Search {
            user_visible_action,
            capability_requirements,
            query,
            ..
        } => {
            query.query.validate("Search query")?;
            if query.extensions.len() > 16 {
                return invalid("Bridge Plan Search has too many filename extensions.");
            }
            for extension in &query.extensions {
                extension.validate("Search filename extension")?;
                if extension.as_str().len() > 16
                    || !extension
                        .as_str()
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                {
                    return invalid("Bridge Plan Search has an invalid filename extension.");
                }
            }
            if has_duplicate_ids(query.extensions.iter().map(|extension| extension.as_str())) {
                return invalid("Bridge Plan Search has duplicate filename extensions.");
            }
            if query.safe_scope_labels.len() > MAX_SAFE_SCOPE_LABELS {
                return invalid("Bridge Plan Search has too many safe scope labels.");
            }
            for scope in &query.safe_scope_labels {
                scope.validate("Search scope")?;
            }
            if has_duplicate_ids(query.safe_scope_labels.iter().map(|scope| scope.as_str())) {
                return invalid("Bridge Plan Search has duplicate safe scope labels.");
            }
            (user_visible_action, capability_requirements)
        }
        BridgePlanStep::Transform {
            user_visible_action,
            capability_requirements,
            input_revision,
            output_revision,
            modification_intent,
            expected_input,
            ..
        } => {
            validate_logical_revision(input_revision, "Transform input revision")?;
            validate_logical_revision(output_revision, "Transform output revision")?;
            if input_revision.logical_object_id != output_revision.logical_object_id
                || output_revision.revision != input_revision.revision.saturating_add(1)
            {
                return invalid("Bridge Plan Transform revision dependency is invalid.");
            }
            modification_intent.validate("Transform modification intent")?;
            validate_contract(expected_input)?;
            (user_visible_action, capability_requirements)
        }
        BridgePlanStep::Transfer {
            user_visible_action,
            capability_requirements,
            ..
        } => (user_visible_action, capability_requirements),
        BridgePlanStep::Execute {
            user_visible_action,
            capability_requirements,
            target_revision,
            execution_intent,
            ..
        } => {
            validate_logical_revision(target_revision, "Execute target revision")?;
            execution_intent.validate("Execute execution intent")?;
            (user_visible_action, capability_requirements)
        }
    };
    action.validate("step action")?;
    if requirements.len() > MAX_CAPABILITY_REQUIREMENTS {
        return invalid("Bridge Plan step has too many capability requirements.");
    }
    for requirement in requirements {
        requirement.category.validate("capability category")?;
        requirement
            .user_visible_requirement
            .validate("capability explanation")?;
    }
    if requirements
        .iter()
        .map(|requirement| {
            (
                requirement.category.as_str(),
                requirement.user_visible_requirement.as_str(),
            )
        })
        .collect::<HashSet<_>>()
        .len()
        != requirements.len()
    {
        return invalid("Bridge Plan step has duplicate capability requirements.");
    }
    Ok(())
}

fn validate_logical_revision(revision: &LogicalObjectRevision, field: &str) -> AppResult<()> {
    id(&revision.logical_object_id, field)?;
    if revision.revision == 0 {
        return invalid("Bridge Plan logical object revision must be positive.");
    }
    Ok(())
}
fn validate_contract(contract: &ObjectContract) -> AppResult<()> {
    contract.object_type.validate("object type")?;
    contract
        .user_visible_description
        .validate("object description")?;
    if contract.media_types.len() > MAX_MEDIA_TYPES {
        return invalid("Object contract has too many media types.");
    }
    for media_type in &contract.media_types {
        media_type.validate("media type")?;
    }
    if has_duplicate_ids(
        contract
            .media_types
            .iter()
            .map(|media_type| media_type.as_str()),
    ) {
        return invalid("Object contract has duplicate media types.");
    }
    Ok(())
}
fn validate_destination(
    destination: &TransferDestination,
    revision: &BridgePlanRevision,
) -> AppResult<()> {
    let device = match destination {
        TransferDestination::PipelineHandoff { device_ref } => device_ref,
        TransferDestination::RequestingDevice { device_ref } => {
            if device_ref != &revision.requesting_device_ref {
                return invalid(
                    "Requesting-device Transfer destination does not match the Bridge requester.",
                );
            }
            device_ref
        }
        TransferDestination::SelectedDevice { device_ref } => {
            if device_ref != &revision.selected_device_ref {
                return invalid(
                    "Selected-device Transfer destination does not match the selected device.",
                );
            }
            device_ref
        }
        TransferDestination::UserSelectedLocation {
            device_ref,
            user_visible_location_scope,
        } => {
            user_visible_location_scope.validate("user-selected location scope")?;
            device_ref
        }
    };
    if !matches_device(device, revision) {
        return invalid("Bridge Plan v1 Transfer destination is outside its Bridge.");
    }
    Ok(())
}
fn matches_device(device: &str, revision: &BridgePlanRevision) -> bool {
    device == revision.requesting_device_ref || device == revision.selected_device_ref
}

/// Validates transfer-start metadata against the receiver's own immutable
/// attempt/revision. This is an admission check only; it does not create an
/// execution grant or expose a private destination path.
pub(crate) fn validate_pipeline_handoff_metadata(
    paths: &AppPaths,
    metadata: &PipelineHandoffMetadata,
    local_device_ref: &str,
    peer_device_ref: &str,
) -> AppResult<()> {
    let attempt = BridgePlanStore::new(paths).list_attempt(&metadata.attempt_id)?;
    if attempt.attempt.bridge_id != metadata.bridge_id
        || attempt.attempt.plan_id != metadata.plan_id
        || attempt.attempt.revision_id != metadata.revision_id
        || attempt.attempt.revision_hash != metadata.revision_hash
        || metadata.destination_device_ref != local_device_ref
        || metadata.source_device_ref != peer_device_ref
    {
        return invalid("Pipeline handoff crossed its current Bridge binding.");
    }
    let revision = BridgePlanStore::new(paths)
        .get_revision(&metadata.revision_id)?
        .revision;
    let step = revision
        .steps
        .iter()
        .find(|step| step.id() == metadata.step_id)
        .ok_or_else(|| AppError::InvalidInput("Pipeline handoff step is unavailable.".into()))?;
    let BridgePlanStep::Transfer {
        destination,
        source_device_ref,
        output_slots,
        ..
    } = step
    else {
        return invalid("Pipeline handoff step is not Transfer.");
    };
    if !matches!(destination, TransferDestination::PipelineHandoff { device_ref } if device_ref == local_device_ref)
        || source_device_ref.as_deref() != Some(peer_device_ref)
        || output_slots.len() != 1
        || output_slots[0]
            .object
            .media_types
            .iter()
            .any(|media| media.as_str() != metadata.media_type)
    {
        return invalid("Pipeline handoff metadata does not match the immutable step.");
    }
    Ok(())
}
fn validate_acyclic(steps: &[BridgePlanStep]) -> AppResult<()> {
    fn visit<'a>(
        id: &'a str,
        graph: &HashMap<&'a str, &'a [String]>,
        visiting: &mut HashSet<&'a str>,
        done: &mut HashSet<&'a str>,
    ) -> bool {
        if done.contains(id) {
            return true;
        }
        if !visiting.insert(id) {
            return false;
        }
        let valid = graph.get(id).is_some_and(|dependencies| {
            dependencies
                .iter()
                .all(|dependency| visit(dependency, graph, visiting, done))
        });
        visiting.remove(id);
        if valid {
            done.insert(id);
        }
        valid
    }
    let graph = steps
        .iter()
        .map(|step| (step.id(), step.dependencies()))
        .collect::<HashMap<_, _>>();
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    if steps
        .iter()
        .any(|step| !visit(step.id(), &graph, &mut visiting, &mut done))
    {
        return invalid("Bridge Plan revision contains a dependency cycle.");
    }
    Ok(())
}
fn id(value: &str, field: &str) -> AppResult<()> {
    const RESERVED_INTERNAL_PREFIXES: &[&str] = &[
        "object-ref-",
        "consent-",
        "authority-",
        "lease-",
        "worker-",
        "sandbox-",
        "process-",
        "runtime-",
        "command-",
    ];
    if value.is_empty()
        || value.len() > MAX_ID_LEN
        || RESERVED_INTERNAL_PREFIXES
            .iter()
            .any(|prefix| value.to_ascii_lowercase().starts_with(prefix))
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return invalid(&format!("Bridge Plan {field} is invalid."));
    }
    Ok(())
}
fn validate_bounded_text(value: &str, field: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_LEN {
        return invalid(&format!("Bridge Plan {field} is invalid."));
    }
    Ok(())
}
fn has_duplicate_ids<'a>(mut values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    values.any(|value| !seen.insert(value))
}
fn invalid(message: &str) -> AppResult<()> {
    Err(AppError::InvalidInput(message.into()))
}

pub(crate) struct BridgePlanStore<'a> {
    paths: &'a AppPaths,
}
impl<'a> BridgePlanStore<'a> {
    pub(crate) fn new(paths: &'a AppPaths) -> Self {
        Self { paths }
    }
    fn connection(&self) -> AppResult<Connection> {
        connection(self.paths)
    }
    pub(crate) fn create_plan(&self, plan: &BridgePlan, state: BridgePlanState) -> AppResult<()> {
        id(&plan.plan_id, "plan id")?;
        id(&plan.bridge_id, "bridge id")?;
        id(&plan.requesting_device_ref, "requesting device")?;
        if state != BridgePlanState::Draft {
            return invalid("Bridge Plan must be created as a draft.");
        }
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &plan.bridge_id)?;
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plans WHERE bridge_id = ?1",
            &plan.bridge_id,
            MAX_PLANS_PER_BRIDGE,
            "too many plans for this Bridge",
        )?;
        tx.execute("INSERT INTO bridge_plans (plan_id, bridge_id, requesting_device_ref, created_at, state) VALUES (?1, ?2, ?3, ?4, ?5)", params![plan.plan_id, plan.bridge_id, plan.requesting_device_ref, plan.created_at, state.as_str()])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn list_attempt(&self, attempt_id: &str) -> AppResult<AttemptRecord> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let attempt = attempt_row_tx(&tx, attempt_id)?;
        tx.commit()?;
        Ok(attempt)
    }
    pub(crate) fn get_approval(&self, approval_id: &str) -> AppResult<ApprovalRecord> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let approval = approval_row_tx(&tx, approval_id)?;
        tx.commit()?;
        Ok(approval)
    }
    pub(crate) fn get_revision(&self, revision_id: &str) -> AppResult<RevisionRecord> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let revision = revision_row_tx(&tx, revision_id)?;
        tx.commit()?;
        Ok(revision)
    }
    pub(crate) fn transition_plan(&self, plan_id: &str, next: BridgePlanState) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let plan = plan_row_tx(&tx, plan_id)?;
        ensure_active_bridge_tx(&tx, &plan.bridge_id)?;
        let current = plan_state_tx(&tx, plan_id)?;
        if !legal_plan(&current, &next) {
            return invalid("Illegal Bridge Plan transition.");
        }
        let changed = tx.execute(
            "UPDATE bridge_plans SET state = ?1 WHERE plan_id = ?2 AND state = ?3",
            params![next.as_str(), plan_id, current.as_str()],
        )?;
        if changed != 1 {
            return invalid("Bridge Plan transition became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn append_revision(
        &self,
        revision: &BridgePlanRevision,
        state: RevisionState,
        created_at: i64,
    ) -> AppResult<()> {
        validate_revision(revision)?;
        if state != RevisionState::Proposed {
            return invalid("Bridge Plan revision must be appended as proposed.");
        }
        if canonical_revision_hash(revision)? != revision.revision_hash {
            return invalid("Bridge Plan revision hash does not match its semantic payload.");
        }
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &revision.bridge_id)?;
        let plan = plan_row_tx(&tx, &revision.plan_id)?;
        if plan.bridge_id != revision.bridge_id
            || plan.requesting_device_ref != revision.requesting_device_ref
        {
            return invalid("Bridge Plan revision does not belong to its plan Bridge.");
        }
        if let Some(alternative) = &revision.alternative {
            id(
                &alternative.based_on_revision_id,
                "alternative base revision id",
            )?;
            alternative
                .change_explanation
                .validate("alternative change explanation")?;
            if alternative.based_on_revision_id == revision.revision_id {
                return invalid("Bridge Plan alternative cannot be based on itself.");
            }
            let base = revision_row_tx(&tx, &alternative.based_on_revision_id)?;
            if base.revision.plan_id != revision.plan_id
                || base.revision.bridge_id != revision.bridge_id
            {
                return invalid(
                    "Bridge Plan alternative must be based on a revision in the same Bridge Plan.",
                );
            }
        }
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_revisions WHERE plan_id = ?1",
            &revision.plan_id,
            MAX_REVISIONS_PER_PLAN,
            "too many revisions for this plan",
        )?;
        tx.execute("INSERT INTO bridge_plan_revisions (revision_id, plan_id, bridge_id, revision_number, revision_hash, created_at, state, revision_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![revision.revision_id, revision.plan_id, revision.bridge_id, revision.revision_number, revision.revision_hash, created_at, state.as_str(), json(revision)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn transition_revision(
        &self,
        revision_id: &str,
        next: RevisionState,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let revision = revision_row_tx(&tx, revision_id)?;
        ensure_active_bridge_tx(&tx, &revision.revision.bridge_id)?;
        let current = revision_state_tx(&tx, revision_id)?;
        if !legal_revision(&current, &next) {
            return invalid("Illegal Bridge Plan revision transition.");
        }
        let changed = tx.execute(
            "UPDATE bridge_plan_revisions SET state = ?1 WHERE revision_id = ?2 AND state = ?3",
            params![next.as_str(), revision_id, current.as_str()],
        )?;
        if changed != 1 {
            return invalid("Bridge Plan revision transition became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn withdraw_unapproved_revision(&self, revision_id: &str) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let revision = revision_row_tx(&tx, revision_id)?;
        ensure_active_bridge_tx(&tx, &revision.revision.bridge_id)?;
        if revision.state != RevisionState::Available {
            return invalid("Only an available Bridge Plan revision can be withdrawn.");
        }
        let approval_count = tx.query_row(
            "SELECT COUNT(*) FROM bridge_plan_approvals WHERE revision_id = ?1",
            [revision_id],
            |row| row.get::<_, i64>(0),
        )?;
        if approval_count != 0 {
            return invalid("An approved Bridge Plan revision cannot be edited.");
        }
        let changed = tx.execute(
            "UPDATE bridge_plan_revisions SET state = 'withdrawn' WHERE revision_id = ?1 AND state = 'available'",
            [revision_id],
        )?;
        if changed != 1 {
            return invalid("Bridge Plan revision withdrawal became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn create_approval(
        &self,
        approval: &BridgePlanApproval,
        created_at: i64,
    ) -> AppResult<()> {
        validate_approval(approval)?;
        if approval.expires_at <= created_at {
            return invalid("Bridge Plan approval is already expired.");
        }
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &approval.bridge_id)?;
        let plan_state = plan_state_tx(&tx, &approval.plan_id)?;
        if plan_state != BridgePlanState::Open {
            return invalid("Bridge Plan approval requires an open plan.");
        }
        let revision = revision_row_tx(&tx, &approval.revision_id)?;
        if revision.revision.plan_id != approval.plan_id
            || revision.revision.bridge_id != approval.bridge_id
            || revision.revision.revision_hash != approval.revision_hash
            || revision.revision.requesting_device_ref != approval.requester_device_ref
            || revision.revision.selected_device_ref != approval.selected_device_ref
            || revision.state != RevisionState::Available
        {
            return invalid("Bridge Plan approval does not match an available revision.");
        }
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_approvals WHERE revision_id = ?1",
            &approval.revision_id,
            MAX_APPROVALS_PER_REVISION,
            "too many approvals for this revision",
        )?;
        let state = ApprovalState::Valid;
        tx.execute("INSERT INTO bridge_plan_approvals (approval_id, plan_id, revision_id, bridge_id, created_at, state, approval_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![approval.approval_id, approval.plan_id, approval.revision_id, approval.bridge_id, created_at, state.as_str(), json(approval)?])?;
        tx.commit()?;
        Ok(())
    }
    /// Phase 2 admission boundary. The graph is compiled from the stored
    /// immutable revision inside the same transaction that consumes approval
    /// and creates the attempt; callers cannot supply a graph.
    pub(crate) fn create_attempt_from_approval(
        &self,
        attempt_id: &str,
        approval_id: &str,
        created_at: i64,
    ) -> AppResult<BridgePlanAttempt> {
        id(attempt_id, "attempt id")?;
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let approval = approval_row_tx(&tx, approval_id)?;
        ensure_active_bridge_tx(&tx, &approval.approval.bridge_id)?;
        let revision = revision_row_tx(&tx, &approval.approval.revision_id)?;
        if revision.state != RevisionState::Available
            || canonical_revision_hash(&revision.revision)? != revision.revision.revision_hash
            || approval.approval.revision_hash != revision.revision.revision_hash
        {
            return Err(AppError::InvalidInput(
                "Bridge Plan approval does not bind its immutable revision.".into(),
            ));
        }
        if framework_execution_unavailable(&revision.revision) {
            return Err(AppError::InvalidInput(
                "Bridge Plan Transform and Execute framework steps are not currently executable."
                    .into(),
            ));
        }
        if approval.state == ApprovalState::Valid && approval.approval.expires_at <= created_at {
            tx.execute("UPDATE bridge_plan_approvals SET state = 'expired' WHERE approval_id = ?1 AND state = 'valid'", [approval_id])?;
            tx.commit()?;
            return Err(AppError::InvalidInput(
                "Bridge Plan approval expired.".into(),
            ));
        }
        if approval.state != ApprovalState::Valid {
            return Err(AppError::InvalidInput(
                "Bridge Plan approval cannot be consumed.".into(),
            ));
        }
        let graph_projection = compile_graph_projection(&revision.revision)?;
        let attempt = BridgePlanAttempt {
            attempt_id: attempt_id.into(),
            plan_id: revision.revision.plan_id.clone(),
            revision_id: revision.revision.revision_id.clone(),
            revision_hash: revision.revision.revision_hash.clone(),
            approval_id: approval_id.into(),
            bridge_id: revision.revision.bridge_id.clone(),
            graph_projection,
        };
        validate_attempt(&attempt)?;
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_attempts WHERE revision_id = ?1",
            &attempt.revision_id,
            MAX_ATTEMPTS_PER_REVISION,
            "too many attempts for this revision",
        )?;
        if tx.execute("UPDATE bridge_plan_approvals SET state = 'consumed' WHERE approval_id = ?1 AND state = 'valid'", [approval_id])? != 1 {
            return Err(AppError::InvalidInput("Bridge Plan approval cannot be consumed.".into()));
        }
        tx.execute("INSERT INTO bridge_plan_attempts (attempt_id, approval_id, plan_id, revision_id, bridge_id, created_at, state, attempt_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'created', ?7)", params![attempt.attempt_id, attempt.approval_id, attempt.plan_id, attempt.revision_id, attempt.bridge_id, created_at, json(&attempt)?])?;
        for node in &attempt.graph_projection.nodes {
            let state = if node.depends_on_node_ids.is_empty() {
                StepExecutionState::Eligible
            } else {
                StepExecutionState::Pending
            };
            tx.execute("INSERT INTO bridge_plan_attempt_steps (attempt_id, step_id, state, updated_at) VALUES (?1, ?2, ?3, ?4)", params![attempt.attempt_id, node.step_id, state.as_str(), created_at])?;
        }
        tx.commit()?;
        Ok(attempt)
    }
    pub(crate) fn transition_attempt(
        &self,
        attempt_id: &str,
        next: AttemptState,
        at: i64,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let attempt = attempt_row_tx(&tx, attempt_id)?;
        ensure_active_bridge_tx(&tx, &attempt.attempt.bridge_id)?;
        let current = attempt.state;
        if !legal_attempt(&current, &next) {
            return invalid("Illegal Bridge Plan attempt transition.");
        }
        let started = (next == AttemptState::Running).then_some(at);
        let ended = matches!(
            next,
            AttemptState::Interrupted
                | AttemptState::Completed
                | AttemptState::Failed
                | AttemptState::Cancelled
        )
        .then_some(at);
        let changed = tx.execute("UPDATE bridge_plan_attempts SET state = ?1, started_at = COALESCE(started_at, ?2), ended_at = COALESCE(?3, ended_at) WHERE attempt_id = ?4 AND state = ?5", params![next.as_str(), started, ended, attempt_id, current.as_str()])?;
        if changed != 1 {
            return invalid("Bridge Plan attempt transition became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn transition_step(
        &self,
        attempt_id: &str,
        step_id: &str,
        next: StepExecutionState,
        at: i64,
    ) -> AppResult<()> {
        id(attempt_id, "attempt id")?;
        id(step_id, "step id")?;
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let attempt = attempt_row_tx(&tx, attempt_id)?;
        ensure_active_bridge_tx(&tx, &attempt.attempt.bridge_id)?;
        if matches!(
            attempt.state,
            AttemptState::Interrupted
                | AttemptState::Completed
                | AttemptState::Failed
                | AttemptState::Cancelled
                | AttemptState::Burned
        ) {
            return Err(AppError::InvalidInput(
                "Bridge Plan attempt is not live.".into(),
            ));
        }
        let current = attempt
            .steps
            .iter()
            .find(|step| step.step_id == step_id)
            .ok_or_else(|| AppError::NotFound("Bridge Plan step not found.".into()))?;
        if !legal_step(&current.state, &next) {
            return invalid("Illegal Bridge Plan step transition.");
        }
        if current.state == StepExecutionState::Pending || next == StepExecutionState::Authorized {
            ensure_step_dependencies_completed(&attempt, step_id)?;
        }
        if tx.execute("UPDATE bridge_plan_attempt_steps SET state = ?1, updated_at = ?2 WHERE attempt_id = ?3 AND step_id = ?4 AND state = ?5", params![next.as_str(), at, attempt_id, step_id, current.state.as_str()])? != 1 {
            return invalid("Bridge Plan step transition became stale.");
        }
        if next == StepExecutionState::Completed {
            let mut updated_attempt = attempt.clone();
            if let Some(step) = updated_attempt
                .steps
                .iter_mut()
                .find(|step| step.step_id == step_id)
            {
                step.state = StepExecutionState::Completed;
            }
            refresh_eligible_steps_tx(&tx, &updated_attempt, at)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn append_activity(&self, activity: &BridgePlanActivity) -> AppResult<()> {
        validate_activity(activity)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &activity.bridge_id)?;
        ensure_activity_scope_tx(&tx, activity)?;
        if let Some(step_id) = &activity.step_id {
            ensure_step_in_revision_tx(&tx, &activity.revision_id, step_id)?;
        }
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_activities WHERE plan_id = ?1",
            &activity.plan_id,
            MAX_ACTIVITIES_PER_PLAN,
            "too many activities for this plan",
        )?;
        tx.execute("INSERT INTO bridge_plan_activities (activity_id, bridge_id, plan_id, revision_id, attempt_id, occurred_at, activity_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![activity.activity_id, activity.bridge_id, activity.plan_id, activity.revision_id, activity.attempt_id, activity.occurred_at, json(activity)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn append_result(&self, result: &BridgePlanResultSummary) -> AppResult<()> {
        validate_result(result)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &result.bridge_id)?;
        let attempt = attempt_row_tx(&tx, &result.attempt_id)?;
        if attempt.attempt.plan_id != result.plan_id
            || attempt.attempt.revision_id != result.revision_id
            || attempt.attempt.bridge_id != result.bridge_id
        {
            return invalid("Bridge Plan result crosses Bridge scope.");
        }
        ensure_step_in_revision_tx(&tx, &result.revision_id, &result.step_id)?;
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_results WHERE attempt_id = ?1",
            &result.attempt_id,
            MAX_RESULTS_PER_ATTEMPT,
            "too many results for this attempt",
        )?;
        tx.execute("INSERT INTO bridge_plan_results (result_id, bridge_id, plan_id, revision_id, attempt_id, created_at, result_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![result.result_id, result.bridge_id, result.plan_id, result.revision_id, result.attempt_id, result.created_at, json(result)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn list_bridge(&self, bridge_id: &str) -> AppResult<BridgePlanRecords> {
        id(bridge_id, "bridge id")?;
        let conn = self.connection()?;
        let plans = query_json(&conn, "SELECT plan_id, bridge_id, requesting_device_ref, created_at FROM bridge_plans WHERE bridge_id = ?1 ORDER BY created_at", bridge_id, |row| Ok(BridgePlan { plan_id: row.get(0)?, bridge_id: row.get(1)?, requesting_device_ref: row.get(2)?, created_at: row.get(3)? }))?;
        let revisions = query_json(&conn, "SELECT revision_json, state, created_at FROM bridge_plan_revisions WHERE bridge_id = ?1 ORDER BY created_at", bridge_id, |row| Ok(RevisionRecord { revision: parse(row.get::<_, String>(0)?)?, state: revision_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)? }))?;
        let approvals = query_json(&conn, "SELECT approval_json, state, created_at FROM bridge_plan_approvals WHERE bridge_id = ?1 ORDER BY created_at", bridge_id, |row| Ok(ApprovalRecord { approval: parse(row.get::<_, String>(0)?)?, state: approval_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)? }))?;
        let mut attempts = query_json(&conn, "SELECT attempt_json, state, created_at, started_at, ended_at, interruption_reason FROM bridge_plan_attempts WHERE bridge_id = ?1 ORDER BY created_at", bridge_id, |row| Ok(AttemptRecord { attempt: parse(row.get::<_, String>(0)?)?, state: attempt_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)?, started_at: row.get(3)?, ended_at: row.get(4)?, interruption_reason: row.get::<_, Option<String>>(5)?.map(SafeActivitySummary), steps: Vec::new() }))?;
        for attempt in &mut attempts {
            attempt.steps = step_rows(&conn, &attempt.attempt.attempt_id)?;
        }
        let activities = query_json(&conn, "SELECT activity_json FROM bridge_plan_activities WHERE bridge_id = ?1 ORDER BY occurred_at", bridge_id, |row| parse(row.get::<_, String>(0)?))?;
        let results = query_json(
            &conn,
            "SELECT result_json FROM bridge_plan_results WHERE bridge_id = ?1 ORDER BY created_at",
            bridge_id,
            |row| parse(row.get::<_, String>(0)?),
        )?;
        Ok(BridgePlanRecords {
            plans,
            revisions,
            approvals,
            attempts,
            activities,
            results,
        })
    }
}

pub(crate) fn reconcile_startup(paths: &AppPaths, now: i64) -> AppResult<usize> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let attempts = {
        let mut stmt = tx.prepare("SELECT attempt_id, bridge_id, plan_id, revision_id FROM bridge_plan_attempts WHERE state IN ('created', 'running') AND NOT EXISTS (SELECT 1 FROM burned_bridges WHERE burned_bridges.room_id = bridge_plan_attempts.bridge_id)")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut interrupted_count = 0;
    for (attempt_id, bridge_id, plan_id, revision_id) in &attempts {
        ensure_active_bridge_tx(&tx, bridge_id)?;
        let interrupted = tx.execute(
            "UPDATE bridge_plan_attempts SET state = 'interrupted', ended_at = ?1, interruption_reason = ?2 WHERE attempt_id = ?3 AND state IN ('created', 'running')",
            params![now, "application_restarted", attempt_id],
        )?;
        if interrupted != 1 {
            continue;
        }
        interrupted_count += 1;
        let activity = BridgePlanActivity {
            activity_id: format!("restart-interrupt:{attempt_id}"),
            bridge_id: bridge_id.clone(),
            plan_id: plan_id.clone(),
            revision_id: revision_id.clone(),
            attempt_id: Some(attempt_id.clone()),
            step_id: None,
            kind: ActivityKind::AttemptInterrupted,
            occurred_at: now,
            summary: "Execution interrupted because Pastey restarted.".into(),
        };
        let activity_exists: i64 = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM bridge_plan_activities WHERE activity_id = ?1)",
            [activity.activity_id.as_str()],
            |row| row.get(0),
        )?;
        let activity_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM bridge_plan_activities WHERE plan_id = ?1",
            [plan_id],
            |row| row.get(0),
        )?;
        if activity_exists == 0 && activity_count >= MAX_ACTIVITIES_PER_PLAN {
            drop_delete_guards(&tx)?;
            tx.execute(
                "DELETE FROM bridge_plan_activities WHERE activity_id = (SELECT activity_id FROM bridge_plan_activities WHERE plan_id = ?1 ORDER BY (activity_id LIKE 'restart-interrupt:%'), occurred_at, activity_id LIMIT 1)",
                [plan_id],
            )?;
            create_delete_guards(&tx)?;
        }
        tx.execute("INSERT OR IGNORE INTO bridge_plan_activities (activity_id, bridge_id, plan_id, revision_id, attempt_id, occurred_at, activity_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![activity.activity_id, activity.bridge_id, activity.plan_id, activity.revision_id, activity.attempt_id, activity.occurred_at, json(&activity)?])?;
    }
    tx.commit()?;
    Ok(interrupted_count)
}
pub(crate) fn delete_bridge_records(paths: &AppPaths, bridge_id: &str) -> AppResult<()> {
    let mut conn = connection(paths)?;
    let tx = conn.transaction()?;
    let burned: i64 = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM burned_bridges WHERE room_id = ?1)",
        [bridge_id],
        |row| row.get(0),
    )?;
    if burned == 0 {
        return invalid("Bridge Plan deletion requires a burned Bridge.");
    }
    drop_delete_guards(&tx)?;
    protocol::delete_bridge_records(&tx, bridge_id)?;
    tx.execute("DELETE FROM bridge_plans WHERE bridge_id = ?1", [bridge_id])?;
    create_delete_guards(&tx)?;
    tx.commit()?;
    Ok(())
}

/// The Bridge Plan database handle is intentionally module-private.  Product
/// modules interact through the repository methods above, never raw SQL.
fn connection(paths: &AppPaths) -> AppResult<Connection> {
    let conn = Connection::open(&paths.db_path)?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(conn)
}

fn plan_row_tx(tx: &Transaction<'_>, plan_id: &str) -> AppResult<BridgePlan> {
    tx.query_row("SELECT plan_id, bridge_id, requesting_device_ref, created_at FROM bridge_plans WHERE plan_id = ?1", [plan_id], |row| Ok(BridgePlan { plan_id: row.get(0)?, bridge_id: row.get(1)?, requesting_device_ref: row.get(2)?, created_at: row.get(3)? })).optional()?.ok_or_else(|| AppError::NotFound("Bridge Plan not found.".into()))
}
fn revision_row_tx(tx: &Transaction<'_>, revision_id: &str) -> AppResult<RevisionRecord> {
    tx.query_row(
        "SELECT revision_json, state, created_at FROM bridge_plan_revisions WHERE revision_id = ?1",
        [revision_id],
        |row| {
            Ok(RevisionRecord {
                revision: parse(row.get::<_, String>(0)?)?,
                state: revision_state_from(&row.get::<_, String>(1)?)
                    .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound("Bridge Plan revision not found.".into()))
}
fn approval_row_tx(tx: &Transaction<'_>, approval_id: &str) -> AppResult<ApprovalRecord> {
    tx.query_row(
        "SELECT approval_json, state, created_at FROM bridge_plan_approvals WHERE approval_id = ?1",
        [approval_id],
        |row| {
            Ok(ApprovalRecord {
                approval: parse(row.get::<_, String>(0)?)?,
                state: approval_state_from(&row.get::<_, String>(1)?)
                    .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound("Bridge Plan approval not found.".into()))
}
fn attempt_row_tx(tx: &Transaction<'_>, attempt_id: &str) -> AppResult<AttemptRecord> {
    let mut record = tx.query_row("SELECT attempt_json, state, created_at, started_at, ended_at, interruption_reason FROM bridge_plan_attempts WHERE attempt_id = ?1", [attempt_id], |row| Ok(AttemptRecord { attempt: parse(row.get::<_, String>(0)?)?, state: attempt_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)?, started_at: row.get(3)?, ended_at: row.get(4)?, interruption_reason: row.get::<_, Option<String>>(5)?.map(SafeActivitySummary), steps: Vec::new() })).optional()?.ok_or_else(|| AppError::NotFound("Bridge Plan attempt not found.".into()))?;
    record.steps = step_rows_tx(tx, attempt_id)?;
    Ok(record)
}

fn step_state_from(value: &str) -> Option<StepExecutionState> {
    Some(match value {
        "pending" => StepExecutionState::Pending,
        "eligible" => StepExecutionState::Eligible,
        "authorized" => StepExecutionState::Authorized,
        "running" => StepExecutionState::Running,
        "completed" => StepExecutionState::Completed,
        "failed" => StepExecutionState::Failed,
        "cancelled" => StepExecutionState::Cancelled,
        _ => return None,
    })
}
fn step_rows(conn: &Connection, attempt_id: &str) -> AppResult<Vec<StepExecutionProjection>> {
    let mut stmt = conn.prepare("SELECT step_id, state, updated_at FROM bridge_plan_attempt_steps WHERE attempt_id = ?1 ORDER BY step_id")?;
    let rows = stmt
        .query_map([attempt_id], |row| {
            Ok(StepExecutionProjection {
                attempt_id: attempt_id.into(),
                step_id: row.get(0)?,
                state: step_state_from(&row.get::<_, String>(1)?)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
fn step_rows_tx(tx: &Transaction<'_>, attempt_id: &str) -> AppResult<Vec<StepExecutionProjection>> {
    let mut stmt = tx.prepare("SELECT step_id, state, updated_at FROM bridge_plan_attempt_steps WHERE attempt_id = ?1 ORDER BY step_id")?;
    let rows = stmt
        .query_map([attempt_id], |row| {
            Ok(StepExecutionProjection {
                attempt_id: attempt_id.into(),
                step_id: row.get(0)?,
                state: step_state_from(&row.get::<_, String>(1)?)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
fn plan_state_tx(tx: &Transaction<'_>, plan_id: &str) -> AppResult<BridgePlanState> {
    tx.query_row(
        "SELECT state FROM bridge_plans WHERE plan_id = ?1",
        [plan_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .and_then(|value| plan_state_from(&value))
    .ok_or_else(|| AppError::NotFound("Bridge Plan not found.".into()))
}
fn revision_state_tx(tx: &Transaction<'_>, revision_id: &str) -> AppResult<RevisionState> {
    tx.query_row(
        "SELECT state FROM bridge_plan_revisions WHERE revision_id = ?1",
        [revision_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .and_then(|value| revision_state_from(&value))
    .ok_or_else(|| AppError::NotFound("Bridge Plan revision not found.".into()))
}
fn plan_state_from(value: &str) -> Option<BridgePlanState> {
    match value {
        "draft" => Some(BridgePlanState::Draft),
        "open" => Some(BridgePlanState::Open),
        "cancelled" => Some(BridgePlanState::Cancelled),
        "burned" => Some(BridgePlanState::Burned),
        _ => None,
    }
}
fn revision_state_from(value: &str) -> Option<RevisionState> {
    match value {
        "proposed" => Some(RevisionState::Proposed),
        "available" => Some(RevisionState::Available),
        "superseded" => Some(RevisionState::Superseded),
        "withdrawn" => Some(RevisionState::Withdrawn),
        "burned" => Some(RevisionState::Burned),
        _ => None,
    }
}
fn approval_state_from(value: &str) -> Option<ApprovalState> {
    match value {
        "awaiting_receiver" => Some(ApprovalState::AwaitingReceiver),
        "valid" => Some(ApprovalState::Valid),
        "denied" => Some(ApprovalState::Denied),
        "expired" => Some(ApprovalState::Expired),
        "consumed" => Some(ApprovalState::Consumed),
        "revoked" => Some(ApprovalState::Revoked),
        "burned" => Some(ApprovalState::Burned),
        _ => None,
    }
}
fn attempt_state_from(value: &str) -> Option<AttemptState> {
    match value {
        "created" => Some(AttemptState::Created),
        "running" => Some(AttemptState::Running),
        "interrupted" => Some(AttemptState::Interrupted),
        "completed" => Some(AttemptState::Completed),
        "failed" => Some(AttemptState::Failed),
        "cancelled" => Some(AttemptState::Cancelled),
        "burned" => Some(AttemptState::Burned),
        _ => None,
    }
}
fn legal_plan(current: &BridgePlanState, next: &BridgePlanState) -> bool {
    matches!(
        (current, next),
        (
            BridgePlanState::Draft,
            BridgePlanState::Open | BridgePlanState::Burned
        ) | (
            BridgePlanState::Open,
            BridgePlanState::Cancelled | BridgePlanState::Burned
        ) | (BridgePlanState::Cancelled, BridgePlanState::Burned)
    )
}
fn legal_revision(current: &RevisionState, next: &RevisionState) -> bool {
    matches!(
        (current, next),
        (
            RevisionState::Proposed,
            RevisionState::Available
                | RevisionState::Withdrawn
                | RevisionState::Superseded
                | RevisionState::Burned
        ) | (
            RevisionState::Available,
            RevisionState::Withdrawn | RevisionState::Superseded | RevisionState::Burned
        ) | (
            RevisionState::Superseded | RevisionState::Withdrawn,
            RevisionState::Burned
        )
    )
}
fn legal_attempt(current: &AttemptState, next: &AttemptState) -> bool {
    matches!(
        (current, next),
        (
            AttemptState::Created,
            AttemptState::Running
                | AttemptState::Failed
                | AttemptState::Cancelled
                | AttemptState::Interrupted
                | AttemptState::Burned
        ) | (
            AttemptState::Running,
            AttemptState::Completed
                | AttemptState::Failed
                | AttemptState::Cancelled
                | AttemptState::Interrupted
                | AttemptState::Burned
        ) | (
            AttemptState::Interrupted,
            AttemptState::Cancelled | AttemptState::Burned
        ) | (
            AttemptState::Completed | AttemptState::Failed | AttemptState::Cancelled,
            AttemptState::Burned
        )
    )
}
fn legal_step(current: &StepExecutionState, next: &StepExecutionState) -> bool {
    matches!(
        (current, next),
        (
            StepExecutionState::Pending,
            StepExecutionState::Eligible | StepExecutionState::Cancelled
        ) | (
            StepExecutionState::Eligible,
            StepExecutionState::Authorized | StepExecutionState::Cancelled
        ) | (
            StepExecutionState::Authorized,
            StepExecutionState::Running | StepExecutionState::Cancelled
        ) | (
            StepExecutionState::Running,
            StepExecutionState::Completed
                | StepExecutionState::Failed
                | StepExecutionState::Cancelled
        )
    )
}
fn ensure_step_dependencies_completed(attempt: &AttemptRecord, step_id: &str) -> AppResult<()> {
    let node = attempt
        .attempt
        .graph_projection
        .nodes
        .iter()
        .find(|node| node.step_id == step_id)
        .ok_or_else(|| AppError::NotFound("Bridge Plan graph step not found.".into()))?;
    for dependency_node in &node.depends_on_node_ids {
        let dependency = attempt
            .attempt
            .graph_projection
            .nodes
            .iter()
            .find(|node| &node.node_id == dependency_node)
            .ok_or_else(|| {
                AppError::InvalidInput("Bridge Plan graph dependency is missing.".into())
            })?;
        if attempt
            .steps
            .iter()
            .find(|step| step.step_id == dependency.step_id)
            .map(|step| &step.state)
            != Some(&StepExecutionState::Completed)
        {
            return invalid("Bridge Plan step dependencies are not complete.");
        }
    }
    Ok(())
}
fn refresh_eligible_steps_tx(
    tx: &Transaction<'_>,
    attempt: &AttemptRecord,
    at: i64,
) -> AppResult<()> {
    for step in &attempt.steps {
        if step.state != StepExecutionState::Pending {
            continue;
        }
        let node = attempt
            .attempt
            .graph_projection
            .nodes
            .iter()
            .find(|node| node.step_id == step.step_id)
            .expect("stored step is graph-bound");
        let ready = node.depends_on_node_ids.iter().all(|dependency_node| {
            attempt
                .attempt
                .graph_projection
                .nodes
                .iter()
                .find(|node| &node.node_id == dependency_node)
                .and_then(|dependency| {
                    attempt
                        .steps
                        .iter()
                        .find(|step| step.step_id == dependency.step_id)
                })
                .is_some_and(|step| step.state == StepExecutionState::Completed)
        });
        if ready {
            tx.execute("UPDATE bridge_plan_attempt_steps SET state = 'eligible', updated_at = ?1 WHERE attempt_id = ?2 AND step_id = ?3 AND state = 'pending'", params![at, attempt.attempt.attempt_id, step.step_id])?;
        }
    }
    Ok(())
}
fn validate_approval(approval: &BridgePlanApproval) -> AppResult<()> {
    for (value, label) in [
        (&approval.approval_id, "approval id"),
        (&approval.plan_id, "plan id"),
        (&approval.revision_id, "revision id"),
        (&approval.bridge_id, "bridge id"),
        (&approval.requester_device_ref, "requesting device"),
        (&approval.selected_device_ref, "selected device"),
    ] {
        id(value, label)?;
    }
    if approval.expires_at <= 0 || !approval.revision_hash.starts_with(HASH_VERSION) {
        return invalid("Bridge Plan approval is invalid.");
    }
    Ok(())
}
fn validate_attempt(attempt: &BridgePlanAttempt) -> AppResult<()> {
    for (value, label) in [
        (&attempt.attempt_id, "attempt id"),
        (&attempt.plan_id, "plan id"),
        (&attempt.revision_id, "revision id"),
        (&attempt.approval_id, "approval id"),
        (&attempt.bridge_id, "bridge id"),
        (&attempt.graph_projection.graph_id, "graph id"),
    ] {
        id(value, label)?;
    }
    if attempt.graph_projection.derived_from_revision_hash != attempt.revision_hash
        || !attempt.revision_hash.starts_with(HASH_VERSION)
    {
        return invalid("Bridge Plan attempt graph is not derived from its revision.");
    }
    for node in &attempt.graph_projection.nodes {
        id(&node.node_id, "graph node id")?;
        id(&node.step_id, "graph step id")?;
        if node.depends_on_node_ids.len() > MAX_GRAPH_DEPENDENCIES
            || node.input_slots.len() > MAX_SLOTS_PER_STEP
            || node.output_slots.len() > MAX_SLOTS_PER_STEP
        {
            return invalid("Bridge Plan graph node exceeds bounded references.");
        }
    }
    Ok(())
}
fn validate_graph_projection(
    graph: &SafeGraphProjection,
    revision: &BridgePlanRevision,
) -> AppResult<()> {
    if graph.derived_from_revision_hash != revision.revision_hash
        || graph.nodes.len() != revision.steps.len()
        || graph.graph_hash != canonical_graph_hash(graph)?
    {
        return invalid("Bridge Plan graph does not exactly represent its revision.");
    }
    let nodes = graph
        .nodes
        .iter()
        .map(|node| (node.step_id.as_str(), node))
        .collect::<HashMap<_, _>>();
    if nodes.len() != graph.nodes.len() {
        return invalid("Bridge Plan graph has duplicate step nodes.");
    }
    let node_ids = graph
        .nodes
        .iter()
        .map(|node| (node.step_id.as_str(), node.node_id.as_str()))
        .collect::<HashMap<_, _>>();
    for step in &revision.steps {
        let Some(node) = nodes.get(step.id()) else {
            return invalid("Bridge Plan graph is missing a revision step.");
        };
        if node.operation != step.operation()
            || node.node_id != format!("revision-step:{}", step.id())
            || node.step != *step
            || node.input_slots != step.inputs()
            || node.output_slots != step.outputs()
            || node.source_device_ref.as_deref() != step.source_device()
            || node.execution_device_ref != step.execution_device()
        {
            return invalid("Bridge Plan graph operation does not match its revision step.");
        }
        let expected_dependencies = step
            .dependencies()
            .iter()
            .map(|dependency| {
                node_ids.get(dependency.as_str()).copied().ok_or_else(|| {
                    AppError::InvalidInput("Bridge Plan graph is missing a dependency node.".into())
                })
            })
            .collect::<AppResult<HashSet<_>>>()?;
        if node.depends_on_node_ids.len()
            != node
                .depends_on_node_ids
                .iter()
                .collect::<HashSet<_>>()
                .len()
        {
            return invalid("Bridge Plan graph has duplicate dependency nodes.");
        }
        if has_duplicate_ids(node.input_slots.iter().map(|slot| slot.slot_id.as_str()))
            || has_duplicate_ids(node.output_slots.iter().map(|slot| slot.slot_id.as_str()))
        {
            return invalid("Bridge Plan graph has duplicate slot references.");
        }
        if node
            .depends_on_node_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>()
            != expected_dependencies
        {
            return invalid("Bridge Plan graph dependency does not match its revision step.");
        }
    }
    Ok(())
}
fn validate_activity(activity: &BridgePlanActivity) -> AppResult<()> {
    for (value, label) in [
        (&activity.activity_id, "activity id"),
        (&activity.bridge_id, "bridge id"),
        (&activity.plan_id, "plan id"),
        (&activity.revision_id, "revision id"),
    ] {
        id(value, label)?;
    }
    activity.summary.validate("activity summary")
}
fn validate_result(result: &BridgePlanResultSummary) -> AppResult<()> {
    for (value, label) in [
        (&result.result_id, "result id"),
        (&result.bridge_id, "bridge id"),
        (&result.plan_id, "plan id"),
        (&result.revision_id, "revision id"),
        (&result.attempt_id, "attempt id"),
        (&result.step_id, "step id"),
    ] {
        id(value, label)?;
    }
    result.status.validate("result status")?;
    result.summary.validate("result summary")?;
    if let Some(description) = &result.produced_object_description {
        description.validate("produced object description")?;
    }
    Ok(())
}
fn ensure_activity_scope_tx(tx: &Transaction<'_>, activity: &BridgePlanActivity) -> AppResult<()> {
    let revision = revision_row_tx(tx, &activity.revision_id)?;
    if revision.revision.plan_id != activity.plan_id
        || revision.revision.bridge_id != activity.bridge_id
    {
        return invalid("Bridge Plan activity crosses Bridge scope.");
    }
    if let Some(attempt_id) = &activity.attempt_id {
        let attempt = attempt_row_tx(tx, attempt_id)?;
        if attempt.attempt.plan_id != activity.plan_id
            || attempt.attempt.revision_id != activity.revision_id
            || attempt.attempt.bridge_id != activity.bridge_id
        {
            return invalid("Bridge Plan activity attempt crosses Bridge scope.");
        }
    }
    Ok(())
}
fn ensure_step_in_revision_tx(
    tx: &Transaction<'_>,
    revision_id: &str,
    step_id: &str,
) -> AppResult<()> {
    id(step_id, "step id")?;
    let revision = revision_row_tx(tx, revision_id)?;
    if revision
        .revision
        .steps
        .iter()
        .any(|step| step.id() == step_id)
    {
        Ok(())
    } else {
        invalid("Bridge Plan step does not belong to its revision.")
    }
}
fn ensure_active_bridge_tx(tx: &Transaction<'_>, bridge_id: &str) -> AppResult<()> {
    let burned: i64 = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM burned_bridges WHERE room_id = ?1)",
        [bridge_id],
        |row| row.get(0),
    )?;
    if burned != 0 {
        return invalid("Bridge Plan cannot change after Bridge Burn.");
    }
    Ok(())
}
fn json<T: Serialize>(value: &T) -> AppResult<String> {
    Ok(serde_json::to_string(value)?)
}
fn parse<T: for<'a> Deserialize<'a>>(value: String) -> Result<T, rusqlite::Error> {
    serde_json::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery)
}
fn limit_tx(
    tx: &Transaction<'_>,
    query: &str,
    value: &str,
    maximum: i64,
    message: &str,
) -> AppResult<()> {
    let count: i64 = tx.query_row(query, [value], |row| row.get(0))?;
    if count >= maximum {
        return Err(AppError::InvalidInput(message.into()));
    }
    Ok(())
}
fn query_json<T, F>(conn: &Connection, query: &str, bridge_id: &str, map: F) -> AppResult<Vec<T>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut stmt = conn.prepare(query)?;
    let rows = stmt.query_map([bridge_id], map)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn object_revision(revision: u64) -> LogicalObjectRevision {
        LogicalObjectRevision {
            logical_object_id: "selected_file".into(),
            revision,
        }
    }

    fn search(device: &str) -> ComposedFilePlanBlock {
        ComposedFilePlanBlock::Search {
            execution_device_ref: device.into(),
            filename_hint: "project/example.py".into(),
            extensions: vec!["py".into()],
            safe_scope_labels: vec!["downloads".into()],
        }
    }

    fn transform(device: &str, revision: u64, intent: &str) -> ComposedFilePlanBlock {
        ComposedFilePlanBlock::Transform {
            execution_device_ref: device.into(),
            target_revision: object_revision(revision),
            modification_intent: intent.into(),
        }
    }

    fn transfer(source: &str, destination: &str) -> ComposedFilePlanBlock {
        ComposedFilePlanBlock::Transfer {
            source_device_ref: source.into(),
            destination_device_ref: destination.into(),
            landing: ComposedTransferLanding::PipelinePrivate,
        }
    }

    fn execute(device: &str, revision: u64, intent: &str) -> ComposedFilePlanBlock {
        ComposedFilePlanBlock::Execute {
            execution_device_ref: device.into(),
            target_revision: object_revision(revision),
            execution_intent: intent.into(),
        }
    }

    fn build(blocks: Vec<ComposedFilePlanBlock>) -> AppResult<BridgePlanRevision> {
        build_composed_file_revision(
            "bridge".into(),
            "A".into(),
            "B".into(),
            "Work with example.py".into(),
            blocks,
        )
    }

    fn store_paths() -> AppPaths {
        let root =
            std::env::temp_dir().join(format!("pastey-bridge-plan-store-{}", uuid::Uuid::new_v4()));
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

    fn persist_approved_revision(
        paths: &AppPaths,
        revision: &BridgePlanRevision,
        approval_id: &str,
        now: i64,
    ) {
        crate::storage::init_database(paths).unwrap();
        let store = BridgePlanStore::new(paths);
        store
            .create_plan(
                &BridgePlan {
                    plan_id: revision.plan_id.clone(),
                    bridge_id: revision.bridge_id.clone(),
                    requesting_device_ref: revision.requesting_device_ref.clone(),
                    created_at: now,
                },
                BridgePlanState::Draft,
            )
            .unwrap();
        store
            .append_revision(revision, RevisionState::Proposed, now)
            .unwrap();
        store
            .transition_plan(&revision.plan_id, BridgePlanState::Open)
            .unwrap();
        store
            .transition_revision(&revision.revision_id, RevisionState::Available)
            .unwrap();
        store
            .create_approval(
                &BridgePlanApproval {
                    approval_id: approval_id.into(),
                    plan_id: revision.plan_id.clone(),
                    revision_id: revision.revision_id.clone(),
                    revision_hash: revision.revision_hash.clone(),
                    bridge_id: revision.bridge_id.clone(),
                    requester_device_ref: revision.requesting_device_ref.clone(),
                    selected_device_ref: revision.selected_device_ref.clone(),
                    expires_at: now + 600,
                },
                now,
            )
            .unwrap();
    }

    #[test]
    fn four_primitive_types_are_distinct_and_validate() {
        let revision = build(vec![
            search("B"),
            transfer("B", "A"),
            transform("A", 1, "Use exponential backoff."),
            execute("A", 2, "Validate the modified program."),
        ])
        .unwrap();
        assert!(matches!(revision.steps[0], BridgePlanStep::Search { .. }));
        assert!(matches!(revision.steps[1], BridgePlanStep::Transfer { .. }));
        assert!(matches!(
            revision.steps[2],
            BridgePlanStep::Transform { .. }
        ));
        assert!(matches!(revision.steps[3], BridgePlanStep::Execute { .. }));
        validate_revision(&revision).unwrap();
    }

    #[test]
    fn search_only_and_direct_transfer_plans_remain_valid() {
        let search_revision = build(vec![search("B")]).unwrap();
        assert_eq!(search_revision.steps.len(), 1);
        assert!(matches!(
            search_revision.steps[0],
            BridgePlanStep::Search { .. }
        ));

        let direct = build_direct_file_transfer_revision(
            "bridge".into(),
            "A".into(),
            "B".into(),
            "Send one file".into(),
        )
        .unwrap();
        assert!(matches!(direct.steps[0], BridgePlanStep::Transfer { .. }));
        validate_revision(&direct).unwrap();
    }

    #[test]
    fn transform_is_generic_intent_and_advances_logical_revision_without_movement() {
        let revision = build(vec![
            search("B"),
            transform("B", 1, "Change retry behavior to exponential backoff."),
        ])
        .unwrap();
        let BridgePlanStep::Transform {
            source_device_ref,
            execution_device_ref,
            input_revision,
            output_revision,
            modification_intent,
            capability_requirements,
            ..
        } = &revision.steps[1]
        else {
            panic!("expected Transform");
        };
        assert_eq!(source_device_ref.as_deref(), Some("B"));
        assert_eq!(execution_device_ref, "B");
        assert_eq!(input_revision, &object_revision(1));
        assert_eq!(output_revision, &object_revision(2));
        assert_eq!(
            modification_intent.0,
            "Change retry behavior to exponential backoff."
        );
        assert!(capability_requirements.is_empty());
        assert!(!revision
            .steps
            .iter()
            .any(|step| matches!(step, BridgePlanStep::Transfer { .. })));
    }

    #[test]
    fn no_transform_step_means_no_mutation_intent_or_revision_advance() {
        let revision = build(vec![
            search("B"),
            execute("B", 1, "Inspect program behavior."),
        ])
        .unwrap();
        assert!(!revision
            .steps
            .iter()
            .any(|step| matches!(step, BridgePlanStep::Transform { .. })));
        let BridgePlanStep::Execute {
            target_revision, ..
        } = &revision.steps[1]
        else {
            panic!("expected Execute");
        };
        assert_eq!(target_revision, &object_revision(1));
    }

    #[test]
    fn execute_is_generic_intent_bound_to_the_current_revision() {
        let revision = build(vec![
            search("B"),
            transform("B", 1, "Modify the selected source."),
            execute(
                "B",
                2,
                "Run or validate the modified object and report the result.",
            ),
        ])
        .unwrap();
        let BridgePlanStep::Execute {
            target_revision,
            execution_intent,
            execution_device_ref,
            capability_requirements,
            ..
        } = &revision.steps[2]
        else {
            panic!("expected Execute");
        };
        assert_eq!(target_revision, &object_revision(2));
        assert_eq!(execution_device_ref, "B");
        assert_eq!(
            execution_intent.0,
            "Run or validate the modified object and report the result."
        );
        assert!(capability_requirements.is_empty());
    }

    #[test]
    fn cross_device_transform_and_execute_require_explicit_transfer() {
        assert!(build(vec![search("B"), transform("A", 1, "Modify it.")]).is_err());
        assert!(build(vec![search("B"), execute("A", 1, "Run it.")]).is_err());
        assert!(build(vec![
            search("B"),
            transfer("B", "A"),
            transform("A", 1, "Modify it."),
            execute("A", 2, "Run it."),
        ])
        .is_ok());
    }

    #[test]
    fn wrong_or_stale_logical_revisions_fail_closed() {
        assert!(build(vec![search("B"), transform("B", 2, "Modify it.")]).is_err());
        assert!(build(vec![
            search("B"),
            transform("B", 1, "Modify it."),
            execute("B", 1, "Run it."),
        ])
        .is_err());
    }

    #[test]
    fn same_host_search_transform_execute_contains_no_hidden_transfer() {
        let revision = build(vec![
            search("B"),
            transform("B", 1, "Modify it."),
            execute("B", 2, "Run it."),
        ])
        .unwrap();
        assert_eq!(revision.steps.len(), 3);
        assert!(!revision
            .steps
            .iter()
            .any(|step| matches!(step, BridgePlanStep::Transfer { .. })));
    }

    #[test]
    fn explicit_movement_is_preserved_as_pipeline_private_only_when_consumed() {
        let revision = build(vec![
            search("B"),
            transfer("B", "A"),
            transform("A", 1, "Modify it."),
            execute("A", 2, "Run it."),
        ])
        .unwrap();
        let BridgePlanStep::Transfer { destination, .. } = &revision.steps[1] else {
            panic!("expected Transfer");
        };
        assert!(
            matches!(destination, TransferDestination::PipelineHandoff { device_ref } if device_ref == "A")
        );
        assert!(build(vec![search("B"), transfer("B", "A")]).is_err());
    }

    #[test]
    fn semantic_intent_edits_change_the_immutable_revision_hash() {
        let first = build(vec![search("B"), transform("B", 1, "Use fixed backoff.")]).unwrap();
        let second = build(vec![
            search("B"),
            transform("B", 1, "Use exponential backoff."),
        ])
        .unwrap();
        assert_ne!(first.revision_hash, second.revision_hash);

        let first = build(vec![search("B"), execute("B", 1, "Run once.")]).unwrap();
        let second = build(vec![
            search("B"),
            execute("B", 1, "Run and report validation."),
        ])
        .unwrap();
        assert_ne!(first.revision_hash, second.revision_hash);
    }

    #[test]
    fn generic_framework_steps_serialize_without_worker_or_runtime_schema() {
        let revision = build(vec![
            search("B"),
            transform("B", 1, "Modify it."),
            execute("B", 2, "Run it."),
        ])
        .unwrap();
        let json = serde_json::to_string(&revision).unwrap();
        for forbidden in [
            "capabilityId",
            "runtimeCapabilityId",
            "startByte",
            "replacementText",
            "interpreter",
            "shell",
            "command",
            "timeoutMs",
        ] {
            assert!(
                !json.contains(forbidden),
                "unexpected concrete field: {forbidden}"
            );
        }
        assert!(json.contains("modification_intent"));
        assert!(json.contains("execution_intent"));
        assert!(framework_execution_unavailable(&revision));
        assert!(!framework_execution_unavailable(
            &build(vec![search("B")]).unwrap()
        ));
    }

    #[test]
    fn store_rejects_framework_only_attempts_without_consuming_approval_or_creating_state() {
        let revisions = [
            build(vec![search("B"), transform("B", 1, "Modify it.")]).unwrap(),
            build(vec![search("B"), execute("B", 1, "Run it.")]).unwrap(),
        ];
        for (index, revision) in revisions.iter().enumerate() {
            let paths = store_paths();
            let now = crate::storage::now_ts();
            let approval_id = format!("approval-{index}");
            let attempt_id = format!("attempt-{index}");
            persist_approved_revision(&paths, revision, &approval_id, now);
            let store = BridgePlanStore::new(&paths);

            let error = store
                .create_attempt_from_approval(&attempt_id, &approval_id, now)
                .unwrap_err();
            assert!(error.to_string().contains("not currently executable"));
            assert_eq!(
                store.get_approval(&approval_id).unwrap().state,
                ApprovalState::Valid
            );
            assert!(store.list_attempt(&attempt_id).is_err());

            let conn = connection(&paths).unwrap();
            let attempt_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM bridge_plan_attempts", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let step_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM bridge_plan_attempt_steps",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(attempt_count, 0);
            assert_eq!(step_count, 0);
        }
    }
}

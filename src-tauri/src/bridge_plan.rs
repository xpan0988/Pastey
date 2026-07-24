//! Durable, Host-owned Bridge Plan foundation.
//!
//! This module intentionally has no Tauri command and is not connected to the
//! current Ask Bridge UI or TaskGraph executor.  It stores safe workspace
//! history only; all capability grants, ObjectRef backing, leases, and process
//! state remain in their existing ephemeral Host-owned stores.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Mutex,
};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::storage;
use crate::{
    error::{AppError, AppResult},
    storage::AppPaths,
};

mod protocol;
pub(crate) use protocol::{
    accept_inbound_protocol_event, attempt_search_result_payload, attempt_start_payload,
    attempt_update_payload, consume_search_execution_grant, consume_transfer_execution_grant,
    consume_transform_execution_grant, protocol_metadata, receiver_decision_payload,
    receiver_review_decision, reconcile_protocol_startup,
    review_request_payload, search_selection_payload, transfer_start_payload,
    transfer_update_payload, transform_start_payload, transform_update_payload,
    ProtocolSearchAuthorityStore,
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
durable_text!(TransformIntentText);
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
durable_text_as_str!(TransformIntentText);
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
bounded_text_from!(TransformIntentText);
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReceiverDecision {
    Approved,
    Denied,
}
impl ReceiverDecision {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
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
    ReceiverDenied,
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
    /// A completed Transform may deliberately remain on the device that
    /// produced it. This is not a network transfer and must never be lowered
    /// into a fake requester return.
    LeaveOnProducingDevice {
        device_ref: String,
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
        intent: TransformIntentText,
        expected_input: ObjectContract,
        expected_output: ObjectContract,
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
}

impl BridgePlanStep {
    fn id(&self) -> &str {
        match self {
            Self::Search { step_id, .. }
            | Self::Transform { step_id, .. }
            | Self::Transfer { step_id, .. } => step_id,
        }
    }
    fn dependencies(&self) -> &[String] {
        match self {
            Self::Search { depends_on, .. }
            | Self::Transform { depends_on, .. }
            | Self::Transfer { depends_on, .. } => depends_on,
        }
    }
    fn inputs(&self) -> &[PlanSlot] {
        match self {
            Self::Search { input_slots, .. }
            | Self::Transform { input_slots, .. }
            | Self::Transfer { input_slots, .. } => input_slots,
        }
    }
    fn outputs(&self) -> &[PlanSlot] {
        match self {
            Self::Search { output_slots, .. }
            | Self::Transform { output_slots, .. }
            | Self::Transfer { output_slots, .. } => output_slots,
        }
    }
    fn operation(&self) -> StepOperation {
        match self {
            Self::Search { .. } => StepOperation::Search,
            Self::Transform { .. } => StepOperation::Transform,
            Self::Transfer { .. } => StepOperation::Transfer,
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

/// Builds the only currently executable file-search revision from bounded
/// product intent. The renderer never supplies a revision, object reference,
/// device binding, or execution grant.
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
        user_visible_description: GeneratedUserVisibleText::from_semantic("one file chosen on the requesting device"),
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
                "After both devices approve this plan, choose one file on the requesting device and transfer it to the selected device.",
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

/// Builds the supported file-based Bridge Plan shapes from bounded product
/// intent. The Host, rather than the renderer or provider, fixes device
/// bindings, object slots, selection rules, and executable semantics.
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

/// Constructs the supported file Transform revision. It carries natural
/// language intent only—never a worker, command, path, runtime, or
/// implementation choice. The selected Host resolves the capability only
/// after it receives the bounded local input.
pub(crate) fn build_file_transform_revision(
    bridge_id: String,
    requesting_device_ref: String,
    selected_device_ref: String,
    original_user_goal: String,
    filename_hint: String,
    extensions: Vec<String>,
    safe_scope_labels: Vec<String>,
    transform_intent: String,
    transfer_to_requester: bool,
) -> AppResult<BridgePlanRevision> {
    let mut revision = build_file_plan_revision(
        bridge_id,
        requesting_device_ref,
        selected_device_ref,
        original_user_goal,
        filename_hint,
        extensions,
        safe_scope_labels,
        transfer_to_requester,
    )?;
    let selected_file = "selected_file".to_owned();
    let transformed_file = "transformed_file".to_owned();
    let transformed_contract = ObjectContract {
        object_type: GeneratedUserVisibleText::from_semantic("file"),
        media_types: vec![GeneratedUserVisibleText::from_semantic("text/plain")],
        user_visible_description: GeneratedUserVisibleText::from_semantic(
            "readable text produced from the selected file",
        ),
    };
    let file_contract = revision
        .steps
        .iter()
        .find_map(|step| match step {
            BridgePlanStep::Search { output_slots, .. } => {
                output_slots.first().map(|slot| slot.object.clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            AppError::InvalidInput("Bridge Plan Search output is unavailable.".into())
        })?;
    let search = revision
        .steps
        .iter_mut()
        .find(|step| matches!(step, BridgePlanStep::Search { .. }))
        .ok_or_else(|| AppError::InvalidInput("Bridge Plan Search step is unavailable.".into()))?;
    let BridgePlanStep::Search { selection, .. } = search else {
        unreachable!()
    };
    *selection = Some(BoundedSearchSelectionRule {
        source_slot_id: "found".into(),
        result_set_limit: 10,
        allowed_object: file_contract.clone(),
        downstream_slot_id: selected_file.clone(),
    });
    revision.search_selection_mode = SearchSelectionMode::BoundedInline;
    let transform = BridgePlanStep::Transform {
        step_id: "transform".into(),
        depends_on: vec!["search".into()],
        input_slots: vec![PlanSlot {
            slot_id: selected_file.clone(),
            object: file_contract.clone(),
            cardinality: SlotCardinality::One,
        }],
        output_slots: vec![PlanSlot {
            slot_id: transformed_file.clone(),
            object: transformed_contract.clone(),
            cardinality: SlotCardinality::One,
        }],
        source_device_ref: Some(revision.selected_device_ref.clone()),
        execution_device_ref: revision.selected_device_ref.clone(),
        user_visible_action: GeneratedUserVisibleText::from_semantic(
            "Process the selected file on the selected device.",
        ),
        capability_requirements: vec![CapabilityRequirement {
            category: GeneratedUserVisibleText::from_semantic("object_transform"),
            user_visible_requirement: GeneratedUserVisibleText::from_semantic(
                "Use a supported local file-processing capability.",
            ),
        }],
        failure_behavior: StepFailureBehavior::StopPlan,
        intent: transform_intent.into(),
        expected_input: file_contract.clone(),
        expected_output: transformed_contract.clone(),
    };
    if let Some(position) = revision
        .steps
        .iter()
        .position(|step| matches!(step, BridgePlanStep::Transfer { .. }))
    {
        let transfer = &mut revision.steps[position];
        if let BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            source,
            ..
        } = transfer
        {
            *depends_on = vec!["transform".into()];
            *input_slots = vec![PlanSlot {
                slot_id: transformed_file.clone(),
                object: transformed_contract.clone(),
                cardinality: SlotCardinality::One,
            }];
            *source = ObjectSelectionRule::FromSlot {
                slot_id: transformed_file.clone(),
            };
        }
        revision.steps.insert(position, transform);
        revision.presentation.step_explanations.insert(
            position,
            StepExplanation {
                step_id: "transform".into(),
                action_summary: GeneratedUserVisibleText::from_semantic(
                    "Process the selected file.",
                ),
                expected_result: GeneratedUserVisibleText::from_semantic(
                    "A processed file ready for the next approved step.",
                ),
            },
        );
    } else {
        revision.steps.push(transform);
        revision
            .presentation
            .step_explanations
            .push(StepExplanation {
                step_id: "transform".into(),
                action_summary: GeneratedUserVisibleText::from_semantic(
                    "Process the selected file.",
                ),
                expected_result: GeneratedUserVisibleText::from_semantic(
                    "A processed file ready for review.",
                ),
            });
    }
    revision.presentation.title =
        GeneratedUserVisibleText::from_semantic("Search and process a file on the selected device");
    revision.presentation.natural_language_plan = GeneratedUserVisibleText::from_semantic("Search the selected device, select one matching file, then process it with a supported local capability.");
    revision.expected_outcome = GeneratedUserVisibleText::from_semantic(
        "A selected file is processed on the selected device.",
    );
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
    pub(crate) receiver_required: bool,
    pub(crate) expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ApprovalRecord {
    pub(crate) approval: BridgePlanApproval,
    pub(crate) state: ApprovalState,
    pub(crate) created_at: i64,
    pub(crate) receiver_decision: Option<ReceiverDecision>,
    pub(crate) receiver_reviewed_at: Option<i64>,
    pub(crate) receiver_evidence: Option<ReceiverDecisionEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReceiverDecisionEvidence {
    pub(crate) revision_hash: String,
    pub(crate) receiver_device_ref: String,
    pub(crate) decision: ReceiverDecision,
    pub(crate) reviewed_at: i64,
    /// A digest or attestation reference, never a raw consent token.
    pub(crate) evidence_digest: String,
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

#[derive(Clone, Debug)]
struct EphemeralStepAuthority {
    authority_id: String,
    bridge_id: String,
    plan_id: String,
    revision_id: String,
    revision_hash: String,
    approval_id: String,
    attempt_id: String,
    step_id: String,
    operation: StepOperation,
    source_device_ref: Option<String>,
    execution_device_ref: String,
    destination_device_ref: Option<String>,
    input_slot_ids: Vec<String>,
    output_slot_ids: Vec<String>,
    object_selection_digest: String,
    transform_contract_digest: String,
    transfer_destination_digest: String,
    expires_at: i64,
    consumed: bool,
}

#[derive(Default)]
pub(crate) struct EphemeralStepAuthorityStore {
    grants: Mutex<HashMap<String, EphemeralStepAuthority>>,
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
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'bridge_plan_approvals')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0 {
        ensure_approval_columns(conn)?;
    }
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
            receiver_decision TEXT, receiver_reviewed_at INTEGER,
            receiver_revision_hash TEXT, receiver_device_ref TEXT,
            receiver_evidence_digest TEXT, approval_json TEXT NOT NULL,
            FOREIGN KEY(plan_id) REFERENCES bridge_plans(plan_id) ON DELETE CASCADE,
            FOREIGN KEY(revision_id) REFERENCES bridge_plan_revisions(revision_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS bridge_plan_receiver_decisions (
            approval_id TEXT PRIMARY KEY,
            revision_hash TEXT NOT NULL,
            receiver_device_ref TEXT NOT NULL,
            decision TEXT NOT NULL CHECK(decision IN ('approved', 'denied')),
            reviewed_at INTEGER NOT NULL,
            evidence_digest TEXT NOT NULL,
            FOREIGN KEY(approval_id) REFERENCES bridge_plan_approvals(approval_id) ON DELETE CASCADE
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
        CREATE TRIGGER IF NOT EXISTS bridge_plan_receiver_evidence_immutable
        BEFORE UPDATE OF receiver_decision, receiver_reviewed_at, receiver_revision_hash, receiver_device_ref, receiver_evidence_digest
        ON bridge_plan_approvals
        WHEN OLD.receiver_decision IS NOT NULL
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan receiver evidence is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_approval_state_guard
        BEFORE UPDATE OF state ON bridge_plan_approvals
        WHEN NOT (
            (OLD.state = 'awaiting_receiver' AND NEW.state IN ('valid', 'denied', 'expired', 'revoked', 'burned')) OR
            (OLD.state = 'valid' AND NEW.state IN ('consumed', 'expired', 'revoked', 'burned')) OR
            (OLD.state IN ('denied', 'expired', 'revoked', 'consumed') AND NEW.state = 'burned')
        )
        BEGIN SELECT RAISE(ABORT, 'Illegal Bridge Plan approval transition'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_receiver_decision_guard
        BEFORE UPDATE OF state ON bridge_plan_approvals
        WHEN OLD.state = 'awaiting_receiver' AND NEW.state IN ('valid', 'denied') AND (
            NEW.receiver_decision IS NULL OR
            NEW.receiver_reviewed_at IS NULL OR
            NEW.receiver_revision_hash IS NULL OR
            NEW.receiver_device_ref IS NULL OR
            NEW.receiver_evidence_digest IS NULL OR
            NOT EXISTS(
                SELECT 1 FROM bridge_plan_receiver_decisions
                WHERE approval_id = NEW.approval_id
                  AND revision_hash = NEW.receiver_revision_hash
                  AND receiver_device_ref = NEW.receiver_device_ref
                  AND decision = NEW.receiver_decision
                  AND reviewed_at = NEW.receiver_reviewed_at
                  AND evidence_digest = NEW.receiver_evidence_digest
            ) OR
            NEW.receiver_revision_hash != json_extract(NEW.approval_json, '$.revision_hash') OR
            NEW.receiver_device_ref != json_extract(NEW.approval_json, '$.selected_device_ref') OR
            (NEW.state = 'valid' AND NEW.receiver_decision != 'approved') OR
            (NEW.state = 'denied' AND NEW.receiver_decision != 'denied')
        )
        BEGIN SELECT RAISE(ABORT, 'Receiver approval requires exact decision evidence'); END;
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
        CREATE TRIGGER IF NOT EXISTS bridge_plan_receiver_decision_insert_guard
        BEFORE INSERT ON bridge_plan_receiver_decisions
        WHEN NOT EXISTS(
            SELECT 1 FROM bridge_plan_approvals
            WHERE approval_id = NEW.approval_id
              AND state IN ('awaiting_receiver', 'valid', 'denied')
              AND json_extract(approval_json, '$.revision_hash') = NEW.revision_hash
              AND json_extract(approval_json, '$.selected_device_ref') = NEW.receiver_device_ref
        )
        BEGIN SELECT RAISE(ABORT, 'Receiver decision must bind an awaiting exact approval'); END;
        CREATE TRIGGER IF NOT EXISTS bridge_plan_receiver_decision_update_immutable
        BEFORE UPDATE ON bridge_plan_receiver_decisions
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan receiver decision is immutable'); END;
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
    ensure_approval_columns(conn)?;
    backfill_receiver_decision_records(conn)?;
    // Remove the former writable marker escape hatch before installing the
    // permanent guards.  Only the private Burn repository temporarily lifts
    // these guards within its own transaction.
    drop_delete_guards(conn)?;
    conn.execute("DROP TABLE IF EXISTS bridge_plan_burn_deletions", [])?;
    create_delete_guards(conn)?;
    protocol::init_schema(conn)?;
    Ok(())
}

fn ensure_approval_columns(conn: &Connection) -> AppResult<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(bridge_plan_approvals)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<HashSet<_>, _>>()?;
    for (column, definition) in [
        ("receiver_decision", "TEXT"),
        ("receiver_revision_hash", "TEXT"),
        ("receiver_device_ref", "TEXT"),
        ("receiver_evidence_digest", "TEXT"),
    ] {
        if !columns.contains(column) {
            conn.execute(
                &format!("ALTER TABLE bridge_plan_approvals ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn backfill_receiver_decision_records(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO bridge_plan_receiver_decisions (approval_id, revision_hash, receiver_device_ref, decision, reviewed_at, evidence_digest) SELECT approval_id, receiver_revision_hash, receiver_device_ref, receiver_decision, receiver_reviewed_at, receiver_evidence_digest FROM bridge_plan_approvals WHERE state IN ('valid', 'denied') AND receiver_decision IS NOT NULL AND receiver_reviewed_at IS NOT NULL AND receiver_revision_hash IS NOT NULL AND receiver_device_ref IS NOT NULL AND receiver_evidence_digest IS NOT NULL",
        [],
    )?;
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
        CREATE TRIGGER IF NOT EXISTS bridge_plan_receiver_decision_delete_guard
        BEFORE DELETE ON bridge_plan_receiver_decisions
        BEGIN SELECT RAISE(ABORT, 'Bridge Plan receiver decisions are removed only by scoped Burn'); END;
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
        BridgePlanStep::Transform {
            expected_input,
            expected_output,
            ..
        } => {
            expected_input.media_types.sort();
            expected_output.media_types.sort();
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
            validate_contract(&selection.allowed_object)?;
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
            intent,
            expected_input,
            expected_output,
            ..
        } => {
            intent.validate("Transform intent")?;
            validate_contract(expected_input)?;
            validate_contract(expected_output)?;
            (user_visible_action, capability_requirements)
        }
        BridgePlanStep::Transfer {
            user_visible_action,
            capability_requirements,
            ..
        } => (user_visible_action, capability_requirements),
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
        TransferDestination::LeaveOnProducingDevice { device_ref } => device_ref,
    };
    if !matches_device(device, revision) {
        return invalid("Bridge Plan v1 Transfer destination is outside its Bridge.");
    }
    Ok(())
}
fn matches_device(device: &str, revision: &BridgePlanRevision) -> bool {
    device == revision.requesting_device_ref || device == revision.selected_device_ref
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
    pub(crate) fn get_plan(&self, plan_id: &str) -> AppResult<BridgePlan> {
        let conn = self.connection()?;
        conn.query_row("SELECT plan_id, bridge_id, requesting_device_ref, created_at FROM bridge_plans WHERE plan_id = ?1", [plan_id], |row| Ok(BridgePlan { plan_id: row.get(0)?, bridge_id: row.get(1)?, requesting_device_ref: row.get(2)?, created_at: row.get(3)? })).optional()?.ok_or_else(|| AppError::NotFound("Bridge Plan not found.".into()))
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
    pub(crate) fn append_alternative_revision(
        &self,
        base_revision_id: &str,
        mut alternative: BridgePlanRevision,
        created_at: i64,
    ) -> AppResult<BridgePlanRevision> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let base = revision_row_tx(&tx, base_revision_id)?;
        ensure_active_bridge_tx(&tx, &base.revision.bridge_id)?;
        if alternative.plan_id != base.revision.plan_id
            || alternative.bridge_id != base.revision.bridge_id
            || alternative.revision_id == base_revision_id
            || alternative.revision_number <= base.revision.revision_number
        {
            return Err(AppError::InvalidInput(
                "Bridge Plan alternative does not derive from its base revision.".into(),
            ));
        }
        if alternative
            .alternative
            .as_ref()
            .map(|proposal| proposal.based_on_revision_id.as_str())
            != Some(base_revision_id)
        {
            return Err(AppError::InvalidInput(
                "Bridge Plan alternative must explain its base revision.".into(),
            ));
        }
        alternative.revision_hash = canonical_revision_hash(&alternative)?;
        tx.execute("INSERT INTO bridge_plan_revisions (revision_id, plan_id, bridge_id, revision_number, revision_hash, created_at, state, revision_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'proposed', ?7)", params![alternative.revision_id, alternative.plan_id, alternative.bridge_id, alternative.revision_number, alternative.revision_hash, created_at, json(&alternative)?])?;
        tx.commit()?;
        Ok(alternative)
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
        let state = if approval.receiver_required {
            ApprovalState::AwaitingReceiver
        } else {
            ApprovalState::Valid
        };
        tx.execute("INSERT INTO bridge_plan_approvals (approval_id, plan_id, revision_id, bridge_id, created_at, state, approval_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![approval.approval_id, approval.plan_id, approval.revision_id, approval.bridge_id, created_at, state.as_str(), json(approval)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn decide_receiver(
        &self,
        approval_id: &str,
        evidence: &ReceiverDecisionEvidence,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let approval = approval_row_tx(&tx, approval_id)?;
        ensure_active_bridge_tx(&tx, &approval.approval.bridge_id)?;
        if approval.state != ApprovalState::AwaitingReceiver {
            return invalid("Bridge Plan approval is not awaiting receiver review.");
        }
        validate_receiver_evidence(evidence)?;
        if evidence.revision_hash != approval.approval.revision_hash
            || evidence.receiver_device_ref != approval.approval.selected_device_ref
        {
            return invalid("Bridge Plan receiver evidence does not bind the approved revision.");
        }
        let next = if evidence.decision == ReceiverDecision::Approved {
            ApprovalState::Valid
        } else {
            ApprovalState::Denied
        };
        tx.execute(
            "INSERT INTO bridge_plan_receiver_decisions (approval_id, revision_hash, receiver_device_ref, decision, reviewed_at, evidence_digest) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![approval_id, evidence.revision_hash, evidence.receiver_device_ref, evidence.decision.as_str(), evidence.reviewed_at, evidence.evidence_digest],
        )?;
        let changed = tx.execute("UPDATE bridge_plan_approvals SET state = ?1, receiver_decision = ?2, receiver_reviewed_at = ?3, receiver_revision_hash = ?4, receiver_device_ref = ?5, receiver_evidence_digest = ?6 WHERE approval_id = ?7 AND state = 'awaiting_receiver'", params![next.as_str(), evidence.decision.as_str(), evidence.reviewed_at, evidence.revision_hash, evidence.receiver_device_ref, evidence.evidence_digest, approval_id])?;
        if changed != 1 {
            return invalid("Bridge Plan receiver decision became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn transition_approval(
        &self,
        approval_id: &str,
        next: ApprovalState,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let approval = approval_row_tx(&tx, approval_id)?;
        ensure_active_bridge_tx(&tx, &approval.approval.bridge_id)?;
        let current = approval_state_tx(&tx, approval_id)?;
        if !legal_approval(&current, &next) {
            return invalid("Illegal Bridge Plan approval transition.");
        }
        let changed = tx.execute(
            "UPDATE bridge_plan_approvals SET state = ?1 WHERE approval_id = ?2 AND state = ?3",
            params![next.as_str(), approval_id, current.as_str()],
        )?;
        if changed != 1 {
            return invalid("Bridge Plan approval transition became stale.");
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn consume_approval_create_attempt(
        &self,
        attempt: &BridgePlanAttempt,
        created_at: i64,
    ) -> AppResult<()> {
        validate_attempt(attempt)?;
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        ensure_active_bridge_tx(&tx, &attempt.bridge_id)?;
        let approval = approval_row_tx(&tx, &attempt.approval_id)?;
        let revision = revision_row_tx(&tx, &attempt.revision_id)?;
        if approval.approval.plan_id != attempt.plan_id
            || approval.approval.revision_id != attempt.revision_id
            || approval.approval.revision_hash != attempt.revision_hash
            || approval.approval.bridge_id != attempt.bridge_id
            || revision.revision.revision_hash != attempt.revision_hash
        {
            return invalid("Bridge Plan attempt does not match its approval.");
        }
        validate_graph_projection(&attempt.graph_projection, &revision.revision)?;
        if approval.state == ApprovalState::Valid && approval.approval.expires_at <= created_at {
            let changed = tx.execute(
                "UPDATE bridge_plan_approvals SET state = 'expired' WHERE approval_id = ?1 AND state = 'valid'",
                [attempt.approval_id.as_str()],
            )?;
            if changed != 1 {
                return invalid("Bridge Plan approval cannot be expired.");
            }
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
        limit_tx(
            &tx,
            "SELECT COUNT(*) FROM bridge_plan_attempts WHERE revision_id = ?1",
            &attempt.revision_id,
            MAX_ATTEMPTS_PER_REVISION,
            "too many attempts for this revision",
        )?;
        let consumed = tx.execute("UPDATE bridge_plan_approvals SET state = 'consumed' WHERE approval_id = ?1 AND state = 'valid'", [attempt.approval_id.as_str()])?;
        if consumed != 1 {
            return invalid("Bridge Plan approval cannot be consumed.");
        }
        tx.execute("INSERT INTO bridge_plan_attempts (attempt_id, approval_id, plan_id, revision_id, bridge_id, created_at, state, attempt_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'created', ?7)", params![attempt.attempt_id, attempt.approval_id, attempt.plan_id, attempt.revision_id, attempt.bridge_id, created_at, json(attempt)?])?;
        for node in &attempt.graph_projection.nodes {
            let state = if node.depends_on_node_ids.is_empty() {
                StepExecutionState::Eligible
            } else {
                StepExecutionState::Pending
            };
            tx.execute(
                "INSERT INTO bridge_plan_attempt_steps (attempt_id, step_id, state, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![attempt.attempt_id, node.step_id, state.as_str(), created_at],
            )?;
        }
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
        if approval.approval.receiver_required
            && (approval.receiver_decision != Some(ReceiverDecision::Approved)
                || approval.receiver_evidence.as_ref().is_none_or(|evidence| {
                    evidence.revision_hash != revision.revision.revision_hash
                        || evidence.receiver_device_ref != revision.revision.selected_device_ref
                }))
        {
            return Err(AppError::InvalidInput(
                "Bridge Plan receiver review is incomplete.".into(),
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
        let approvals = query_json(&conn, "SELECT approval_json, state, created_at, receiver_decision, receiver_reviewed_at, receiver_revision_hash, receiver_device_ref, receiver_evidence_digest FROM bridge_plan_approvals WHERE bridge_id = ?1 ORDER BY created_at", bridge_id, |row| Ok(ApprovalRecord { approval: parse(row.get::<_, String>(0)?)?, state: approval_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)?, receiver_decision: receiver_decision_from_column(row.get(3)?)?, receiver_reviewed_at: row.get(4)?, receiver_evidence: receiver_evidence_from_columns(row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)? }))?;
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

impl EphemeralStepAuthorityStore {
    pub(crate) fn derive(
        &self,
        store: &BridgePlanStore<'_>,
        attempt_id: &str,
        step_id: &str,
        now: i64,
        expires_at: i64,
    ) -> AppResult<String> {
        if expires_at <= now {
            return Err(AppError::InvalidInput(
                "Bridge Plan step authority expiry is invalid.".into(),
            ));
        }
        let attempt = store.list_attempt(attempt_id)?;
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
        let step = attempt
            .steps
            .iter()
            .find(|projection| projection.step_id == step_id)
            .ok_or_else(|| AppError::NotFound("Bridge Plan step not found.".into()))?;
        if step.state != StepExecutionState::Eligible {
            return Err(AppError::InvalidInput(
                "Bridge Plan step is not eligible.".into(),
            ));
        }
        ensure_step_dependencies_completed(&attempt, step_id)?;
        let node = attempt
            .attempt
            .graph_projection
            .nodes
            .iter()
            .find(|node| node.step_id == step_id)
            .ok_or_else(|| AppError::NotFound("Bridge Plan graph step not found.".into()))?;
        store.transition_step(attempt_id, step_id, StepExecutionState::Authorized, now)?;
        let authority_id = format!("ephemeral-step-authority:{}", uuid::Uuid::new_v4());
        let digest = |value: Value| {
            format!(
                "sha256:{}",
                blake3::hash(canonical_json(&value).as_bytes()).to_hex()
            )
        };
        let authority = EphemeralStepAuthority {
            authority_id: authority_id.clone(),
            bridge_id: attempt.attempt.bridge_id.clone(),
            plan_id: attempt.attempt.plan_id.clone(),
            revision_id: attempt.attempt.revision_id.clone(),
            revision_hash: attempt.attempt.revision_hash.clone(),
            approval_id: attempt.attempt.approval_id.clone(),
            attempt_id: attempt_id.into(),
            step_id: step_id.into(),
            operation: node.operation.clone(),
            source_device_ref: node.source_device_ref.clone(),
            execution_device_ref: node.execution_device_ref.clone(),
            destination_device_ref: authority_destination_device(&node.step),
            input_slot_ids: node
                .input_slots
                .iter()
                .map(|slot| slot.slot_id.clone())
                .collect(),
            output_slot_ids: node
                .output_slots
                .iter()
                .map(|slot| slot.slot_id.clone())
                .collect(),
            object_selection_digest: digest(
                serde_json::to_value(&node.step).expect("semantic step serializes"),
            ),
            transform_contract_digest: digest(
                serde_json::to_value(&node.step).expect("semantic step serializes"),
            ),
            transfer_destination_digest: digest(
                serde_json::to_value(&node.step).expect("semantic step serializes"),
            ),
            expires_at,
            consumed: false,
        };
        self.grants
            .lock()
            .map_err(|_| {
                AppError::InvalidInput("Bridge Plan authority store is unavailable.".into())
            })?
            .insert(authority_id.clone(), authority);
        Ok(authority_id)
    }
    pub(crate) fn consume(
        &self,
        store: &BridgePlanStore<'_>,
        authority_id: &str,
        now: i64,
    ) -> AppResult<()> {
        let mut grants = self.grants.lock().map_err(|_| {
            AppError::InvalidInput("Bridge Plan authority store is unavailable.".into())
        })?;
        let authority = grants
            .get_mut(authority_id)
            .ok_or_else(|| AppError::NotFound("Bridge Plan authority not found.".into()))?;
        if authority.consumed || authority.expires_at <= now {
            return invalid("Bridge Plan authority is unavailable.");
        }
        let attempt = store.list_attempt(&authority.attempt_id)?;
        let revision = store.get_revision(&authority.revision_id)?;
        if attempt.attempt.bridge_id != authority.bridge_id
            || attempt.attempt.plan_id != authority.plan_id
            || attempt.attempt.revision_id != authority.revision_id
            || attempt.attempt.revision_hash != authority.revision_hash
            || attempt.attempt.approval_id != authority.approval_id
            || revision.revision.revision_hash != authority.revision_hash
            || validate_graph_projection(&attempt.attempt.graph_projection, &revision.revision)
                .is_err()
            || matches!(
                attempt.state,
                AttemptState::Interrupted
                    | AttemptState::Completed
                    | AttemptState::Failed
                    | AttemptState::Cancelled
                    | AttemptState::Burned
            )
        {
            return invalid("Bridge Plan authority is no longer valid.");
        }
        if attempt
            .steps
            .iter()
            .find(|step| step.step_id == authority.step_id)
            .map(|step| &step.state)
            != Some(&StepExecutionState::Authorized)
        {
            return invalid("Bridge Plan authority step is no longer authorized.");
        }
        let node = attempt
            .attempt
            .graph_projection
            .nodes
            .iter()
            .find(|node| node.step_id == authority.step_id)
            .ok_or_else(|| AppError::NotFound("Bridge Plan graph step not found.".into()))?;
        let digest = |value: Value| {
            format!(
                "sha256:{}",
                blake3::hash(canonical_json(&value).as_bytes()).to_hex()
            )
        };
        if authority.authority_id != authority_id
            || authority.operation != node.operation
            || authority.source_device_ref != node.source_device_ref
            || authority.execution_device_ref != node.execution_device_ref
            || authority.destination_device_ref != authority_destination_device(&node.step)
            || authority.input_slot_ids
                != node
                    .input_slots
                    .iter()
                    .map(|slot| slot.slot_id.clone())
                    .collect::<Vec<_>>()
            || authority.output_slot_ids
                != node
                    .output_slots
                    .iter()
                    .map(|slot| slot.slot_id.clone())
                    .collect::<Vec<_>>()
            || authority.object_selection_digest
                != digest(serde_json::to_value(&node.step).expect("semantic step serializes"))
            || authority.transform_contract_digest
                != digest(serde_json::to_value(&node.step).expect("semantic step serializes"))
            || authority.transfer_destination_digest
                != digest(serde_json::to_value(&node.step).expect("semantic step serializes"))
        {
            grants.remove(authority_id);
            return invalid("Bridge Plan authority binding no longer matches its approved step.");
        }
        authority.consumed = true;
        Ok(())
    }
    pub(crate) fn purge_bridge(&self, bridge_id: &str) {
        if let Ok(mut grants) = self.grants.lock() {
            grants.retain(|_, authority| authority.bridge_id != bridge_id);
        }
    }
    fn purge_attempt(&self, attempt_id: &str) {
        if let Ok(mut grants) = self.grants.lock() {
            grants.retain(|_, authority| authority.attempt_id != attempt_id);
        }
    }
    pub(crate) fn transition_attempt(
        &self,
        store: &BridgePlanStore<'_>,
        attempt_id: &str,
        next: AttemptState,
        at: i64,
    ) -> AppResult<()> {
        store.transition_attempt(attempt_id, next.clone(), at)?;
        if matches!(
            next,
            AttemptState::Interrupted
                | AttemptState::Completed
                | AttemptState::Failed
                | AttemptState::Cancelled
                | AttemptState::Burned
        ) {
            self.purge_attempt(attempt_id);
        }
        Ok(())
    }
    pub(crate) fn transition_step(
        &self,
        store: &BridgePlanStore<'_>,
        attempt_id: &str,
        step_id: &str,
        next: StepExecutionState,
        at: i64,
    ) -> AppResult<()> {
        store.transition_step(attempt_id, step_id, next.clone(), at)?;
        if matches!(
            next,
            StepExecutionState::Failed | StepExecutionState::Cancelled
        ) {
            self.purge_attempt(attempt_id);
        }
        Ok(())
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
    tx.query_row("SELECT approval_json, state, created_at, receiver_decision, receiver_reviewed_at, receiver_revision_hash, receiver_device_ref, receiver_evidence_digest FROM bridge_plan_approvals WHERE approval_id = ?1", [approval_id], |row| Ok(ApprovalRecord { approval: parse(row.get::<_, String>(0)?)?, state: approval_state_from(&row.get::<_, String>(1)?).ok_or_else(|| rusqlite::Error::InvalidQuery)?, created_at: row.get(2)?, receiver_decision: receiver_decision_from_column(row.get(3)?)?, receiver_reviewed_at: row.get(4)?, receiver_evidence: receiver_evidence_from_columns(row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)? })).optional()?.ok_or_else(|| AppError::NotFound("Bridge Plan approval not found.".into()))
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
fn approval_state_tx(tx: &Transaction<'_>, approval_id: &str) -> AppResult<ApprovalState> {
    tx.query_row(
        "SELECT state FROM bridge_plan_approvals WHERE approval_id = ?1",
        [approval_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .and_then(|value| approval_state_from(&value))
    .ok_or_else(|| AppError::NotFound("Bridge Plan approval not found.".into()))
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
fn receiver_decision_from(value: &str) -> Option<ReceiverDecision> {
    match value {
        "approved" => Some(ReceiverDecision::Approved),
        "denied" => Some(ReceiverDecision::Denied),
        _ => None,
    }
}
fn receiver_decision_from_column(
    value: Option<String>,
) -> rusqlite::Result<Option<ReceiverDecision>> {
    value
        .map(|value| receiver_decision_from(&value).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()
}
fn receiver_evidence_from_columns(
    decision: Option<String>,
    reviewed_at: Option<i64>,
    revision_hash: Option<String>,
    receiver_device_ref: Option<String>,
    evidence_digest: Option<String>,
) -> rusqlite::Result<Option<ReceiverDecisionEvidence>> {
    match (
        decision,
        reviewed_at,
        revision_hash,
        receiver_device_ref,
        evidence_digest,
    ) {
        (None, None, None, None, None) => Ok(None),
        (
            Some(decision),
            Some(reviewed_at),
            Some(revision_hash),
            Some(receiver_device_ref),
            Some(evidence_digest),
        ) => Ok(Some(ReceiverDecisionEvidence {
            decision: receiver_decision_from(&decision).ok_or(rusqlite::Error::InvalidQuery)?,
            reviewed_at,
            revision_hash,
            receiver_device_ref,
            evidence_digest,
        })),
        _ => Err(rusqlite::Error::InvalidQuery),
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
fn legal_approval(current: &ApprovalState, next: &ApprovalState) -> bool {
    matches!(
        (current, next),
        (
            ApprovalState::AwaitingReceiver,
            ApprovalState::Expired | ApprovalState::Revoked | ApprovalState::Burned
        ) | (
            ApprovalState::Valid,
            ApprovalState::Consumed
                | ApprovalState::Expired
                | ApprovalState::Revoked
                | ApprovalState::Burned
        ) | (
            ApprovalState::Denied
                | ApprovalState::Expired
                | ApprovalState::Revoked
                | ApprovalState::Consumed,
            ApprovalState::Burned
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
fn authority_destination_device(step: &BridgePlanStep) -> Option<String> {
    match step {
        BridgePlanStep::Transfer { destination, .. } => Some(match destination {
            TransferDestination::RequestingDevice { device_ref }
            | TransferDestination::SelectedDevice { device_ref }
            | TransferDestination::UserSelectedLocation { device_ref, .. }
            | TransferDestination::LeaveOnProducingDevice { device_ref } => device_ref.clone(),
        }),
        _ => None,
    }
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
fn validate_receiver_evidence(evidence: &ReceiverDecisionEvidence) -> AppResult<()> {
    id(&evidence.receiver_device_ref, "receiver device")?;
    if !evidence.revision_hash.starts_with(HASH_VERSION)
        || evidence.reviewed_at <= 0
        || evidence.evidence_digest.len() > MAX_ID_LEN
        || !evidence
            .evidence_digest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return invalid("Bridge Plan receiver evidence is invalid.");
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
    use std::sync::{Arc, Barrier};

    fn paths() -> AppPaths {
        let root =
            std::env::temp_dir().join(format!("pastey-bridge-plan-{}", uuid::Uuid::new_v4()));
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
    fn contract() -> ObjectContract {
        ObjectContract {
            object_type: "file".into(),
            media_types: vec!["text/plain".into(), "text/markdown".into()],
            user_visible_description: "a matching document".into(),
        }
    }
    fn revision() -> BridgePlanRevision {
        let search = BridgePlanStep::Search {
            step_id: "search".into(),
            depends_on: vec![],
            input_slots: vec![],
            output_slots: vec![PlanSlot {
                slot_id: "found".into(),
                object: contract(),
                cardinality: SlotCardinality::Many,
            }],
            source_device_ref: Some("selected".into()),
            execution_device_ref: "selected".into(),
            user_visible_action: "Search the selected device for matching documents.".into(),
            capability_requirements: vec![CapabilityRequirement {
                category: "object_search".into(),
                user_visible_requirement: "Search approved locations.".into(),
            }],
            failure_behavior: StepFailureBehavior::StopPlan,
            query: SearchIntent {
                query: "report".into(),
                extensions: vec!["pdf".into()],
                safe_scope_labels: vec!["documents".into(), "downloads".into()],
            },
            selection: Some(BoundedSearchSelectionRule {
                source_slot_id: "found".into(),
                result_set_limit: 10,
                allowed_object: contract(),
                downstream_slot_id: "chosen".into(),
            }),
        };
        let transfer = BridgePlanStep::Transfer {
            step_id: "transfer".into(),
            depends_on: vec!["search".into()],
            input_slots: vec![PlanSlot {
                slot_id: "chosen".into(),
                object: contract(),
                cardinality: SlotCardinality::One,
            }],
            output_slots: vec![],
            source_device_ref: Some("selected".into()),
            execution_device_ref: "selected".into(),
            user_visible_action: "Send the selected document back to this device.".into(),
            capability_requirements: vec![CapabilityRequirement {
                category: "object_transfer".into(),
                user_visible_requirement: "Transfer the selected result.".into(),
            }],
            failure_behavior: StepFailureBehavior::StopPlan,
            source: ObjectSelectionRule::FromSlot {
                slot_id: "chosen".into(),
            },
            destination: TransferDestination::RequestingDevice {
                device_ref: "requester".into(),
            },
        };
        let mut revision = BridgePlanRevision {
            schema_version: "bridge-plan-v1".into(),
            plan_id: "plan".into(),
            revision_id: "revision".into(),
            revision_number: 1,
            revision_hash: String::new(),
            bridge_id: "bridge".into(),
            requesting_device_ref: "requester".into(),
            selected_device_ref: "selected".into(),
            original_user_goal: "Find my report and send it here.".into(),
            presentation: BridgePlanPresentation {
                title: "Find report".into(),
                natural_language_plan:
                    "Search the selected device, let me choose a report, then transfer it here."
                        .into(),
                step_explanations: vec![
                    StepExplanation {
                        step_id: "search".into(),
                        action_summary: "Find matching reports.".into(),
                        expected_result: "A bounded list of matching documents.".into(),
                    },
                    StepExplanation {
                        step_id: "transfer".into(),
                        action_summary: "Transfer the document here.".into(),
                        expected_result: "The selected document arrives on this device.".into(),
                    },
                ],
            },
            expected_outcome: "A selected report is transferred to the requesting device.".into(),
            search_selection_mode: SearchSelectionMode::BoundedInline,
            steps: vec![search, transfer],
            alternative: None,
        };
        revision.revision_hash = canonical_revision_hash(&revision).unwrap();
        revision
    }
    fn transform_revision() -> BridgePlanRevision {
        let mut revision = revision();
        let transform = BridgePlanStep::Transform {
            step_id: "transform".into(),
            depends_on: vec!["search".into()],
            input_slots: vec![PlanSlot {
                slot_id: "chosen".into(),
                object: contract(),
                cardinality: SlotCardinality::One,
            }],
            output_slots: vec![PlanSlot {
                slot_id: "transformed".into(),
                object: contract(),
                cardinality: SlotCardinality::One,
            }],
            source_device_ref: Some("selected".into()),
            execution_device_ref: "selected".into(),
            user_visible_action: "Summarize the selected document.".into(),
            capability_requirements: vec![CapabilityRequirement {
                category: "object_transform".into(),
                user_visible_requirement: "Transform the selected result.".into(),
            }],
            failure_behavior: StepFailureBehavior::StopPlan,
            intent: "Summarize the selected document.".into(),
            expected_input: contract(),
            expected_output: contract(),
        };
        if let BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            source,
            ..
        } = &mut revision.steps[1]
        {
            *depends_on = vec!["transform".into()];
            input_slots[0].slot_id = "transformed".into();
            *source = ObjectSelectionRule::FromSlot {
                slot_id: "transformed".into(),
            };
        }
        revision.steps.insert(1, transform);
        revision.presentation.step_explanations.insert(
            1,
            StepExplanation {
                step_id: "transform".into(),
                action_summary: "Summarize the selected document.".into(),
                expected_result: "A transformed document.".into(),
            },
        );
        revision.revision_hash = canonical_revision_hash(&revision).unwrap();
        revision
    }
    fn plan() -> BridgePlan {
        BridgePlan {
            plan_id: "plan".into(),
            bridge_id: "bridge".into(),
            requesting_device_ref: "requester".into(),
            created_at: 10,
        }
    }
    fn approval(revision: &BridgePlanRevision, id: &str) -> BridgePlanApproval {
        BridgePlanApproval {
            approval_id: id.into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            bridge_id: revision.bridge_id.clone(),
            requester_device_ref: revision.requesting_device_ref.clone(),
            selected_device_ref: revision.selected_device_ref.clone(),
            receiver_required: false,
            expires_at: 100,
        }
    }
    fn receiver_evidence(
        revision: &BridgePlanRevision,
        decision: ReceiverDecision,
        reviewed_at: i64,
    ) -> ReceiverDecisionEvidence {
        ReceiverDecisionEvidence {
            revision_hash: revision.revision_hash.clone(),
            receiver_device_ref: revision.selected_device_ref.clone(),
            decision,
            reviewed_at,
            evidence_digest: "sha256:receiver-review".into(),
        }
    }
    fn attempt(revision: &BridgePlanRevision, approval_id: &str, id: &str) -> BridgePlanAttempt {
        BridgePlanAttempt {
            attempt_id: id.into(),
            plan_id: revision.plan_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_hash: revision.revision_hash.clone(),
            approval_id: approval_id.into(),
            bridge_id: revision.bridge_id.clone(),
            graph_projection: compile_graph_projection(revision).unwrap(),
        }
    }
    fn store() -> (AppPaths, BridgePlanStore<'static>) {
        let paths = Box::new(paths());
        let leaked = Box::leak(paths);
        storage::init_database(leaked).unwrap();
        (leaked.clone(), BridgePlanStore::new(leaked))
    }
    #[test]
    fn schema_migrates_receiver_evidence_columns_before_installing_guards() {
        let paths = paths();
        let conn = Connection::open(&paths.db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE bridge_plan_approvals (approval_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, revision_id TEXT NOT NULL, bridge_id TEXT NOT NULL, created_at INTEGER NOT NULL, state TEXT NOT NULL, receiver_reviewed_at INTEGER, approval_json TEXT NOT NULL);",
        )
        .unwrap();
        storage::init_database(&paths).unwrap();
        let conn = connection(&paths).unwrap();
        let columns = conn
            .prepare("PRAGMA table_info(bridge_plan_approvals)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<HashSet<_>, _>>()
            .unwrap();
        assert!(columns.contains("receiver_decision"));
        assert!(columns.contains("receiver_evidence_digest"));
    }
    fn ready(store: &BridgePlanStore<'_>) -> BridgePlanRevision {
        let revision = revision();
        store.create_plan(&plan(), BridgePlanState::Draft).unwrap();
        store
            .append_revision(&revision, RevisionState::Proposed, 11)
            .unwrap();
        store
            .transition_plan("plan", BridgePlanState::Open)
            .unwrap();
        store
            .transition_revision("revision", RevisionState::Available)
            .unwrap();
        revision
    }

    #[test]
    fn canonical_hash_is_deterministic_and_semantic() {
        let revision = revision();
        assert_eq!(
            canonical_revision_hash(&revision).unwrap(),
            revision.revision_hash
        );
        let equivalent = revision.clone();
        assert_eq!(
            canonical_revision_hash(&equivalent).unwrap(),
            revision.revision_hash
        );
        let mut reordered = revision.clone();
        reordered.steps.reverse();
        assert_ne!(
            canonical_revision_hash(&reordered).unwrap(),
            revision.revision_hash
        );
        let mut changed = revision.clone();
        changed.expected_outcome.0.push('!');
        assert_ne!(
            canonical_revision_hash(&changed).unwrap(),
            revision.revision_hash
        );
        let mut sets = revision.clone();
        if let BridgePlanStep::Search { query, .. } = &mut sets.steps[0] {
            query.safe_scope_labels.reverse();
        }
        assert_eq!(
            canonical_revision_hash(&sets).unwrap(),
            revision.revision_hash
        );

        let mut schema = revision.clone();
        schema.schema_version = "bridge-plan-v2".into();
        assert_ne!(
            canonical_revision_hash(&schema).unwrap(),
            revision.revision_hash
        );
        let mut goal = revision.clone();
        goal.original_user_goal = "Find a different report.".into();
        assert_ne!(
            canonical_revision_hash(&goal).unwrap(),
            revision.revision_hash
        );
        let mut presentation = revision.clone();
        presentation.presentation.title = "Find another report".into();
        assert_ne!(
            canonical_revision_hash(&presentation).unwrap(),
            revision.revision_hash
        );
        let mut explanation = revision.clone();
        explanation.presentation.step_explanations[0].expected_result = "A single result.".into();
        assert_ne!(
            canonical_revision_hash(&explanation).unwrap(),
            revision.revision_hash
        );
        let mut requirement = revision.clone();
        if let BridgePlanStep::Search {
            capability_requirements,
            ..
        } = &mut requirement.steps[0]
        {
            capability_requirements[0].user_visible_requirement = "Search documents only.".into();
        }
        assert_ne!(
            canonical_revision_hash(&requirement).unwrap(),
            revision.revision_hash
        );
        let mut selection = revision.clone();
        if let BridgePlanStep::Search {
            selection: Some(rule),
            ..
        } = &mut selection.steps[0]
        {
            rule.result_set_limit = 9;
        }
        assert_ne!(
            canonical_revision_hash(&selection).unwrap(),
            revision.revision_hash
        );
        let mut destination = revision.clone();
        if let BridgePlanStep::Transfer { destination, .. } = &mut destination.steps[1] {
            *destination = TransferDestination::SelectedDevice {
                device_ref: "selected".into(),
            };
        }
        assert_ne!(
            canonical_revision_hash(&destination).unwrap(),
            revision.revision_hash
        );
        let mut metadata_only = revision.clone();
        metadata_only.revision_id = "another-revision".into();
        metadata_only.revision_number = 2;
        assert_eq!(
            canonical_revision_hash(&metadata_only).unwrap(),
            revision.revision_hash
        );
    }
    #[test]
    fn revision_validation_rejects_invalid_graph_and_private_fields() {
        let mut invalid_revision = revision();
        invalid_revision.presentation.step_explanations.pop();
        assert!(validate_revision(&invalid_revision).is_err());
        let mut cycle = revision();
        if let BridgePlanStep::Search { depends_on, .. } = &mut cycle.steps[0] {
            depends_on.push("transfer".into());
        }
        assert!(validate_revision(&cycle).is_err());
        let mut invalid_selection = revision();
        if let BridgePlanStep::Search {
            selection: Some(selection),
            ..
        } = &mut invalid_selection.steps[0]
        {
            selection.source_slot_id = "not-a-search-output".into();
        }
        assert!(validate_revision(&invalid_selection).is_err());
        let serialized = serde_json::to_string(&revision()).unwrap();
        assert!(!serialized.contains("internal_execution"));
        assert!(!serialized.contains("authority_token"));
    }
    #[test]
    fn lifecycle_transitions_reject_illegal_moves() {
        assert!(legal_plan(&BridgePlanState::Draft, &BridgePlanState::Open));
        assert!(!legal_plan(
            &BridgePlanState::Cancelled,
            &BridgePlanState::Open
        ));
        assert!(legal_revision(
            &RevisionState::Proposed,
            &RevisionState::Available
        ));
        assert!(!legal_revision(
            &RevisionState::Available,
            &RevisionState::Proposed
        ));
        assert!(legal_approval(
            &ApprovalState::Valid,
            &ApprovalState::Consumed
        ));
        assert!(!legal_approval(
            &ApprovalState::Consumed,
            &ApprovalState::Valid
        ));
        assert!(legal_attempt(
            &AttemptState::Created,
            &AttemptState::Interrupted
        ));
        assert!(!legal_attempt(
            &AttemptState::Completed,
            &AttemptState::Running
        ));
    }
    #[test]
    fn store_enforces_immutability_scope_and_one_attempt_per_approval() {
        let (paths, primary_store) = store();
        let revision = ready(&primary_store);
        assert!(primary_store
            .append_revision(&revision, RevisionState::Proposed, 12)
            .is_err());
        assert!(connection(&paths)
            .unwrap()
            .execute(
                "UPDATE bridge_plan_revisions SET revision_json = ?1 WHERE revision_id = ?2",
                params!["{}", "revision"],
            )
            .is_err());
        assert!(connection(&paths)
            .unwrap()
            .execute(
                "DELETE FROM bridge_plan_revisions WHERE revision_id = ?1",
                ["revision"]
            )
            .is_err());
        assert!(connection(&paths)
            .unwrap()
            .execute("DELETE FROM bridge_plans WHERE plan_id = ?1", ["plan"])
            .is_err());
        assert!(connection(&paths)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO bridge_plan_revisions (revision_id, plan_id, bridge_id, revision_number, revision_hash, created_at, state, revision_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'proposed', ?7)",
                params![revision.revision_id, revision.plan_id, revision.bridge_id, revision.revision_number, revision.revision_hash, 11, json(&revision).unwrap()],
            )
            .is_err());
        let approved = approval(&revision, "approval");
        primary_store.create_approval(&approved, 20).unwrap();
        let first = attempt(&revision, "approval", "attempt");
        primary_store
            .consume_approval_create_attempt(&first, 21)
            .unwrap();
        assert!(connection(&paths)
            .unwrap()
            .execute(
                "DELETE FROM bridge_plan_approvals WHERE approval_id = ?1",
                ["approval"]
            )
            .is_err());
        assert!(primary_store
            .consume_approval_create_attempt(&attempt(&revision, "approval", "attempt-two"), 22)
            .is_err());
        let mut cross = approval(&revision, "cross");
        cross.bridge_id = "other".into();
        assert!(primary_store.create_approval(&cross, 20).is_err());
    }
    #[test]
    fn approval_rejection_expiry_and_receiver_review_fail_closed() {
        let (paths, store) = store();
        let revision = ready(&store);
        let mut receiver = approval(&revision, "receiver");
        receiver.receiver_required = true;
        store.create_approval(&receiver, 20).unwrap();
        assert!(store
            .consume_approval_create_attempt(&attempt(&revision, "receiver", "attempt"), 21)
            .is_err());
        store
            .decide_receiver(
                "receiver",
                &receiver_evidence(&revision, ReceiverDecision::Denied, 22),
            )
            .unwrap();
        assert!(store
            .consume_approval_create_attempt(&attempt(&revision, "receiver", "attempt"), 23)
            .is_err());
        let expired = approval(&revision, "expired");
        store.create_approval(&expired, 20).unwrap();
        assert!(store
            .consume_approval_create_attempt(&attempt(&revision, "expired", "expired-attempt"), 101)
            .is_err());
        assert_eq!(
            store
                .list_bridge("bridge")
                .unwrap()
                .approvals
                .iter()
                .find(|record| record.approval.approval_id == "expired")
                .unwrap()
                .state,
            ApprovalState::Expired
        );
        let mut receiver_bypass = approval(&revision, "receiver-bypass");
        receiver_bypass.receiver_required = true;
        store.create_approval(&receiver_bypass, 20).unwrap();
        assert!(store
            .transition_approval("receiver-bypass", ApprovalState::Valid)
            .is_err());
        let conn = connection(&paths).unwrap();
        assert!(conn
            .execute(
                "UPDATE bridge_plan_approvals SET state = 'valid' WHERE approval_id = ?1",
                ["receiver-bypass"],
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE bridge_plan_approvals SET state = 'valid', receiver_decision = 'approved', receiver_reviewed_at = 23, receiver_revision_hash = 'bridge-plan-revision-hash-v1:mismatch', receiver_device_ref = 'selected', receiver_evidence_digest = 'sha256:raw' WHERE approval_id = ?1",
                ["receiver-bypass"],
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE bridge_plan_approvals SET state = 'valid', receiver_decision = 'approved', receiver_reviewed_at = 23, receiver_revision_hash = ?1, receiver_device_ref = 'selected', receiver_evidence_digest = 'sha256:raw' WHERE approval_id = ?2",
                params![revision.revision_hash, "receiver-bypass"],
            )
            .is_err());
        let mut mismatched = receiver_evidence(&revision, ReceiverDecision::Approved, 23);
        mismatched.revision_hash = "bridge-plan-revision-hash-v1:mismatch".into();
        assert!(store
            .decide_receiver("receiver-bypass", &mismatched)
            .is_err());
        let approved_evidence = receiver_evidence(&revision, ReceiverDecision::Approved, 24);
        store
            .decide_receiver("receiver-bypass", &approved_evidence)
            .unwrap();
        assert!(conn
            .execute(
                "UPDATE bridge_plan_approvals SET receiver_evidence_digest = 'sha256:changed' WHERE approval_id = ?1",
                ["receiver-bypass"],
            )
            .is_err());
        let approved_record = store
            .list_bridge("bridge")
            .unwrap()
            .approvals
            .into_iter()
            .find(|record| record.approval.approval_id == "receiver-bypass")
            .unwrap();
        assert_eq!(
            approved_record.receiver_decision,
            Some(ReceiverDecision::Approved)
        );
        assert_eq!(approved_record.receiver_evidence, Some(approved_evidence));
        let revoked = approval(&revision, "revoked");
        store.create_approval(&revoked, 20).unwrap();
        store
            .transition_approval("revoked", ApprovalState::Revoked)
            .unwrap();
        assert!(store
            .consume_approval_create_attempt(&attempt(&revision, "revoked", "revoked-attempt"), 21)
            .is_err());
        let burned = approval(&revision, "burned");
        store.create_approval(&burned, 20).unwrap();
        store
            .transition_approval("burned", ApprovalState::Burned)
            .unwrap();
        assert!(store
            .consume_approval_create_attempt(&attempt(&revision, "burned", "burned-attempt"), 21)
            .is_err());
    }
    #[test]
    fn failed_admission_does_not_consume_a_valid_approval() {
        let (_paths, store) = store();
        let revision = ready(&store);
        let approval = approval(&revision, "approval");
        store.create_approval(&approval, 20).unwrap();
        let mut malformed = attempt(&revision, "approval", "bad-attempt");
        malformed.graph_projection.nodes[1].operation = StepOperation::Transform;
        assert!(store
            .consume_approval_create_attempt(&malformed, 21)
            .is_err());
        store
            .consume_approval_create_attempt(&attempt(&revision, "approval", "good-attempt"), 22)
            .unwrap();
    }
    #[test]
    fn concurrent_consumption_and_transitions_are_compare_and_swap() {
        let (paths, primary_store) = store();
        let revision = ready(&primary_store);
        primary_store
            .create_approval(&approval(&revision, "concurrent"), 20)
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles = ["concurrent-a", "concurrent-b"].map(|attempt_id| {
            let barrier = Arc::clone(&barrier);
            let paths = paths.clone();
            let revision = revision.clone();
            std::thread::spawn(move || {
                barrier.wait();
                BridgePlanStore::new(&paths)
                    .consume_approval_create_attempt(
                        &attempt(&revision, "concurrent", attempt_id),
                        21,
                    )
                    .is_ok()
            })
        });
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let attempt_id = primary_store.list_bridge("bridge").unwrap().attempts[0]
            .attempt
            .attempt_id
            .clone();
        let barrier = Arc::new(Barrier::new(2));
        let handles = [(), ()].map(|_| {
            let barrier = Arc::clone(&barrier);
            let paths = paths.clone();
            let attempt_id = attempt_id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                BridgePlanStore::new(&paths)
                    .transition_attempt(&attempt_id, AttemptState::Running, 22)
                    .is_ok()
            })
        });
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .filter(|ok| *ok)
                .count(),
            1
        );
    }
    #[test]
    fn restart_interrupts_only_live_attempts_and_is_idempotent() {
        let (paths, store) = store();
        let revision = ready(&store);
        for (approval_id, attempt_id) in [("a1", "attempt-created"), ("a2", "attempt-running")] {
            let approval = approval(&revision, approval_id);
            store.create_approval(&approval, 20).unwrap();
            store
                .consume_approval_create_attempt(&attempt(&revision, approval_id, attempt_id), 21)
                .unwrap();
        }
        store
            .transition_attempt("attempt-running", AttemptState::Running, 22)
            .unwrap();
        assert_eq!(reconcile_startup(&paths, 30).unwrap(), 2);
        assert_eq!(reconcile_startup(&paths, 31).unwrap(), 0);
        let records = store.list_bridge("bridge").unwrap();
        assert!(records
            .attempts
            .iter()
            .all(|attempt| attempt.state == AttemptState::Interrupted));
        assert_eq!(
            records
                .activities
                .iter()
                .filter(|activity| activity.kind == ActivityKind::AttemptInterrupted)
                .count(),
            2
        );
    }
    #[test]
    fn restart_cas_preserves_a_terminal_attempt() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "terminal"), 20)
            .unwrap();
        store
            .consume_approval_create_attempt(
                &attempt(&revision, "terminal", "terminal-attempt"),
                21,
            )
            .unwrap();
        connection(&paths)
            .unwrap()
            .execute(
                "UPDATE bridge_plan_attempts SET state = 'completed', ended_at = 22 WHERE attempt_id = ?1 AND state = 'created'",
                ["terminal-attempt"],
            )
            .unwrap();
        assert_eq!(reconcile_startup(&paths, 30).unwrap(), 0);
        let records = store.list_bridge("bridge").unwrap();
        assert_eq!(records.attempts[0].state, AttemptState::Completed);
        assert!(records.activities.is_empty());
    }
    #[test]
    fn restart_preserves_valid_unconsumed_approval() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "valid"), 20)
            .unwrap();
        reconcile_startup(&paths, 30).unwrap();
        let records = store.list_bridge("bridge").unwrap();
        assert_eq!(records.approvals[0].state, ApprovalState::Valid);
    }
    #[test]
    fn result_summaries_are_safe_and_bridge_bound() {
        let (_paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "approval"), 20)
            .unwrap();
        store
            .consume_approval_create_attempt(&attempt(&revision, "approval", "attempt"), 21)
            .unwrap();
        let result = BridgePlanResultSummary {
            result_id: "result".into(),
            bridge_id: "bridge".into(),
            plan_id: "plan".into(),
            revision_id: "revision".into(),
            attempt_id: "attempt".into(),
            step_id: "transfer".into(),
            status: "completed".into(),
            summary: "Document transferred.".into(),
            produced_object_description: Some("A document was transferred.".into()),
            created_at: 22,
        };
        store.append_result(&result).unwrap();
        let mut cross = result.clone();
        cross.result_id = "cross-result".into();
        cross.bridge_id = "other".into();
        assert!(store.append_result(&cross).is_err());
        assert_eq!(store.list_bridge("bridge").unwrap().results.len(), 1);
    }
    #[test]
    fn burn_delete_is_complete_and_retryable_after_failure() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_plan(
                &BridgePlan {
                    plan_id: "other-plan".into(),
                    bridge_id: "other-bridge".into(),
                    requesting_device_ref: "requester".into(),
                    created_at: 10,
                },
                BridgePlanState::Draft,
            )
            .unwrap();
        let approval = approval(&revision, "approval");
        store.create_approval(&approval, 20).unwrap();
        store
            .consume_approval_create_attempt(&attempt(&revision, "approval", "attempt"), 21)
            .unwrap();
        let activity = BridgePlanActivity {
            activity_id: "activity".into(),
            bridge_id: "bridge".into(),
            plan_id: "plan".into(),
            revision_id: "revision".into(),
            attempt_id: Some("attempt".into()),
            step_id: None,
            kind: ActivityKind::AttemptCreated,
            occurred_at: 22,
            summary: "Attempt created.".into(),
        };
        store.append_activity(&activity).unwrap();
        let conn = connection(&paths).unwrap();
        conn.execute(
            "INSERT INTO burned_bridges (room_id, burned_at) VALUES (?1, ?2)",
            params!["bridge", 23],
        )
        .unwrap();
        conn.execute_batch("CREATE TRIGGER fail_bridge_plan_delete BEFORE DELETE ON bridge_plans BEGIN SELECT RAISE(ABORT, 'fail'); END;").unwrap();
        assert!(delete_bridge_records(&paths, "bridge").is_err());
        conn.execute_batch("DROP TRIGGER fail_bridge_plan_delete;")
            .unwrap();
        delete_bridge_records(&paths, "bridge").unwrap();
        assert!(store.list_bridge("bridge").unwrap().plans.is_empty());
        assert_eq!(
            store.get_plan("other-plan").unwrap().bridge_id,
            "other-bridge"
        );
    }
    #[test]
    fn authority_cutoff_rejects_further_bridge_plan_mutation() {
        let (paths, store) = store();
        let revision = ready(&store);
        let conn = connection(&paths).unwrap();
        conn.execute(
            "INSERT INTO burned_bridges (room_id, burned_at) VALUES (?1, ?2)",
            params!["bridge", 30],
        )
        .unwrap();
        assert!(store
            .create_approval(&approval(&revision, "after-burn"), 31)
            .is_err());
        assert!(store
            .append_activity(&BridgePlanActivity {
                activity_id: "after-burn-activity".into(),
                bridge_id: "bridge".into(),
                plan_id: "plan".into(),
                revision_id: "revision".into(),
                attempt_id: None,
                step_id: None,
                kind: ActivityKind::RevisionProposed,
                occurred_at: 31,
                summary: "Must not be stored.".into(),
            })
            .is_err());
        assert!(conn
            .execute(
                "UPDATE bridge_plan_revisions SET state = 'superseded' WHERE revision_id = ?1",
                ["revision"],
            )
            .is_err());
        let raw = approval(&revision, "after-burn-raw");
        assert!(conn
            .execute(
                "INSERT INTO bridge_plan_approvals (approval_id, plan_id, revision_id, bridge_id, created_at, state, approval_json) VALUES (?1, ?2, ?3, ?4, ?5, 'valid', ?6)",
                params![raw.approval_id, raw.plan_id, raw.revision_id, raw.bridge_id, 31, json(&raw).unwrap()],
            )
            .is_err());
    }
    #[test]
    fn bounded_storage_rejects_uncontrolled_activity_growth() {
        let (_paths, store) = store();
        let _revision = ready(&store);
        for index in 0..MAX_ACTIVITIES_PER_PLAN {
            let activity = BridgePlanActivity {
                activity_id: format!("a-{index}"),
                bridge_id: "bridge".into(),
                plan_id: "plan".into(),
                revision_id: "revision".into(),
                attempt_id: None,
                step_id: None,
                kind: ActivityKind::RevisionProposed,
                occurred_at: index,
                summary: "bounded activity".into(),
            };
            store.append_activity(&activity).unwrap();
        }
        let next = BridgePlanActivity {
            activity_id: "overflow".into(),
            bridge_id: "bridge".into(),
            plan_id: "plan".into(),
            revision_id: "revision".into(),
            attempt_id: None,
            step_id: None,
            kind: ActivityKind::RevisionProposed,
            occurred_at: 2_000,
            summary: "overflow".into(),
        };
        assert!(store.append_activity(&next).is_err());
    }
    #[test]
    fn restart_reserves_or_replaces_activity_capacity_for_interruption() {
        let (paths, store) = store();
        let revision = ready(&store);
        for index in 0..MAX_ACTIVITIES_PER_PLAN {
            store
                .append_activity(&BridgePlanActivity {
                    activity_id: format!("capacity-{index}"),
                    bridge_id: "bridge".into(),
                    plan_id: "plan".into(),
                    revision_id: "revision".into(),
                    attempt_id: None,
                    step_id: None,
                    kind: ActivityKind::RevisionProposed,
                    occurred_at: index,
                    summary: "bounded activity".into(),
                })
                .unwrap();
        }
        store
            .create_approval(&approval(&revision, "restart-capacity"), 20)
            .unwrap();
        store
            .consume_approval_create_attempt(
                &attempt(&revision, "restart-capacity", "restart-attempt"),
                21,
            )
            .unwrap();
        assert_eq!(reconcile_startup(&paths, 30).unwrap(), 1);
        let records = store.list_bridge("bridge").unwrap();
        assert_eq!(records.activities.len() as i64, MAX_ACTIVITIES_PER_PLAN);
        assert!(records
            .activities
            .iter()
            .any(|activity| activity.activity_id == "restart-interrupt:restart-attempt"));
    }
    #[test]
    fn typed_durable_text_preserves_user_prose_and_excludes_internal_types() {
        let raw = RawUserGoal::from("Run rm -rf /tmp only if I explicitly approve it.");
        assert!(raw.validate("raw goal").is_ok());
        let generated = GeneratedUserVisibleText::from_semantic("Search approved documents.");
        assert!(generated.validate("generated plan").is_ok());
        assert!(id("object-ref-private", "plan id").is_err());
        assert!(id("worker-runtime", "step id").is_err());
        assert!(
            TransformIntentText::from("Summarize the selected document.")
                .validate("test")
                .is_ok()
        );
        assert!(SafeLocationDescription::from("Downloads folder")
            .validate("test")
            .is_ok());
    }
    #[test]
    fn rejects_oversized_nested_collections_and_unknown_step_references() {
        let mut oversized = revision();
        if let BridgePlanStep::Search { query, .. } = &mut oversized.steps[0] {
            query.safe_scope_labels = (0..=MAX_SAFE_SCOPE_LABELS)
                .map(|index| SafeLocationDescription::from(format!("scope-{index}")))
                .collect();
        }
        assert!(validate_revision(&oversized).is_err());
        let mut duplicate_dependencies = revision();
        if let BridgePlanStep::Transfer { depends_on, .. } = &mut duplicate_dependencies.steps[1] {
            depends_on.push("search".into());
        }
        assert!(validate_revision(&duplicate_dependencies).is_err());
        let mut duplicate_scope = revision();
        if let BridgePlanStep::Search { query, .. } = &mut duplicate_scope.steps[0] {
            query
                .safe_scope_labels
                .push(query.safe_scope_labels[0].clone());
        }
        assert!(validate_revision(&duplicate_scope).is_err());
        let mut duplicate_media_type = revision();
        if let BridgePlanStep::Search { output_slots, .. } = &mut duplicate_media_type.steps[0] {
            let media_type = output_slots[0].object.media_types[0].clone();
            output_slots[0].object.media_types.push(media_type);
        }
        assert!(validate_revision(&duplicate_media_type).is_err());
        let mut duplicate_requirement = revision();
        if let BridgePlanStep::Search {
            capability_requirements,
            ..
        } = &mut duplicate_requirement.steps[0]
        {
            capability_requirements.push(capability_requirements[0].clone());
        }
        assert!(validate_revision(&duplicate_requirement).is_err());
        let mut duplicate_input = revision();
        if let BridgePlanStep::Transfer { input_slots, .. } = &mut duplicate_input.steps[1] {
            input_slots.push(input_slots[0].clone());
        }
        assert!(validate_revision(&duplicate_input).is_err());
        let (_paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "approval"), 20)
            .unwrap();
        store
            .consume_approval_create_attempt(&attempt(&revision, "approval", "attempt"), 21)
            .unwrap();
        let mut graph_duplicate_slots = attempt(&revision, "approval", "duplicate-graph");
        let duplicate_slot = graph_duplicate_slots.graph_projection.nodes[1].input_slots[0].clone();
        graph_duplicate_slots.graph_projection.nodes[1]
            .input_slots
            .push(duplicate_slot);
        assert!(
            validate_graph_projection(&graph_duplicate_slots.graph_projection, &revision).is_err()
        );
        let result = BridgePlanResultSummary {
            result_id: "unknown-step".into(),
            bridge_id: "bridge".into(),
            plan_id: "plan".into(),
            revision_id: "revision".into(),
            attempt_id: "attempt".into(),
            step_id: "missing".into(),
            status: "completed".into(),
            summary: "No result.".into(),
            produced_object_description: None,
            created_at: 22,
        };
        assert!(store.append_result(&result).is_err());
    }

    #[test]
    fn phase2_compiles_admits_projects_steps_and_derives_one_use_authority() {
        let (_paths, store) = store();
        let revision = ready(&store);
        let graph = compile_graph_projection(&revision).unwrap();
        assert_eq!(graph.nodes.len(), revision.steps.len());
        assert_eq!(compile_graph_projection(&revision).unwrap(), graph);
        let mut widened = graph.clone();
        widened.nodes[0].execution_device_ref = "requester".into();
        assert!(validate_graph_projection(&widened, &revision).is_err());

        store
            .create_approval(&approval(&revision, "phase2"), 20)
            .unwrap();
        let attempt = store
            .create_attempt_from_approval("phase2-attempt", "phase2", 21)
            .unwrap();
        assert_eq!(attempt.graph_projection, graph);
        let record = store.list_attempt("phase2-attempt").unwrap();
        assert_eq!(
            record
                .steps
                .iter()
                .find(|step| step.step_id == "search")
                .unwrap()
                .state,
            StepExecutionState::Eligible
        );
        assert_eq!(
            record
                .steps
                .iter()
                .find(|step| step.step_id == "transfer")
                .unwrap()
                .state,
            StepExecutionState::Pending
        );

        let authorities = EphemeralStepAuthorityStore::default();
        let authority = authorities
            .derive(&store, "phase2-attempt", "search", 22, 40)
            .unwrap();
        assert!(authorities.consume(&store, &authority, 23).is_ok());
        assert!(authorities.consume(&store, &authority, 24).is_err());
        store
            .transition_step("phase2-attempt", "search", StepExecutionState::Running, 25)
            .unwrap();
        store
            .transition_step(
                "phase2-attempt",
                "search",
                StepExecutionState::Completed,
                26,
            )
            .unwrap();
        assert_eq!(
            store
                .list_attempt("phase2-attempt")
                .unwrap()
                .steps
                .iter()
                .find(|step| step.step_id == "transfer")
                .unwrap()
                .state,
            StepExecutionState::Eligible
        );

        let mut alternative = revision.clone();
        alternative.revision_id = "revision-alternative".into();
        alternative.revision_number = 2;
        alternative.revision_hash.clear();
        alternative.alternative = Some(AlternativeProposal {
            based_on_revision_id: "revision".into(),
            change_explanation: "Use the same selected device with a revised bounded outcome."
                .into(),
        });
        let alternative = store
            .append_alternative_revision("revision", alternative, 30)
            .unwrap();
        assert_ne!(alternative.revision_hash, revision.revision_hash);
        assert!(store
            .create_approval(&approval(&alternative, "alternative-approval"), 31)
            .is_err());
    }

    #[test]
    fn phase2_step_sql_guards_reject_skips_stale_and_post_burn_updates() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "step-sql"), 20)
            .unwrap();
        store
            .create_attempt_from_approval("step-sql-attempt", "step-sql", 21)
            .unwrap();
        let conn = connection(&paths).unwrap();
        assert!(conn.execute("UPDATE bridge_plan_attempt_steps SET state = 'completed' WHERE attempt_id = ?1 AND step_id = 'transfer'", ["step-sql-attempt"]).is_err());
        assert!(conn.execute("UPDATE bridge_plan_attempt_steps SET state = 'running' WHERE attempt_id = ?1 AND step_id = 'search'", ["step-sql-attempt"]).is_err());
        store
            .transition_step(
                "step-sql-attempt",
                "search",
                StepExecutionState::Authorized,
                22,
            )
            .unwrap();
        assert!(store
            .transition_step(
                "step-sql-attempt",
                "search",
                StepExecutionState::Authorized,
                23
            )
            .is_err());
        conn.execute(
            "INSERT INTO burned_bridges (room_id, burned_at) VALUES (?1, ?2)",
            params!["bridge", 24],
        )
        .unwrap();
        assert!(conn.execute("UPDATE bridge_plan_attempt_steps SET state = 'running' WHERE attempt_id = ?1 AND step_id = 'search'", ["step-sql-attempt"]).is_err());
    }

    #[test]
    fn phase2_admission_rejects_each_graph_semantic_mismatch_without_consuming() {
        type GraphMutation = Box<dyn Fn(&mut BridgePlanAttempt)>;
        let cases: Vec<(&str, bool, GraphMutation)> = vec![
            (
                "operation",
                true,
                Box::new(|a| a.graph_projection.nodes[0].operation = StepOperation::Transfer),
            ),
            (
                "source device",
                true,
                Box::new(|a| {
                    a.graph_projection.nodes[0].source_device_ref = Some("requester".into())
                }),
            ),
            (
                "execution device",
                true,
                Box::new(|a| a.graph_projection.nodes[0].execution_device_ref = "requester".into()),
            ),
            (
                "input slots",
                true,
                Box::new(|a| a.graph_projection.nodes[1].input_slots[0].slot_id = "found".into()),
            ),
            (
                "output slots",
                true,
                Box::new(|a| a.graph_projection.nodes[0].output_slots[0].slot_id = "other".into()),
            ),
            (
                "dependencies",
                true,
                Box::new(|a| a.graph_projection.nodes[1].depends_on_node_ids.clear()),
            ),
            (
                "object type",
                true,
                Box::new(|a| {
                    a.graph_projection.nodes[0].output_slots[0]
                        .object
                        .object_type = "clipboard".into()
                }),
            ),
            (
                "media types",
                true,
                Box::new(|a| {
                    a.graph_projection.nodes[0].output_slots[0]
                        .object
                        .media_types = vec!["image/png".into()]
                }),
            ),
            (
                "object constraints",
                true,
                Box::new(|a| {
                    a.graph_projection.nodes[0].output_slots[0]
                        .object
                        .user_visible_description = "a different object".into()
                }),
            ),
            (
                "search query",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Search { query, .. } =
                        &mut a.graph_projection.nodes[0].step
                    {
                        query.query = "invoice".into();
                    }
                }),
            ),
            (
                "selection rule",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Search {
                        selection: Some(rule),
                        ..
                    } = &mut a.graph_projection.nodes[0].step
                    {
                        rule.result_set_limit = 9;
                    }
                }),
            ),
            (
                "capability requirements",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Search {
                        capability_requirements,
                        ..
                    } = &mut a.graph_projection.nodes[0].step
                    {
                        capability_requirements[0].category = "different".into();
                    }
                }),
            ),
            (
                "failure behavior",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Search {
                        failure_behavior, ..
                    } = &mut a.graph_projection.nodes[0].step
                    {
                        *failure_behavior = StepFailureBehavior::RequireNewRevision;
                    }
                }),
            ),
            (
                "transfer source",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Transfer { source, .. } =
                        &mut a.graph_projection.nodes[1].step
                    {
                        *source = ObjectSelectionRule::FromSlot {
                            slot_id: "found".into(),
                        };
                    }
                }),
            ),
            (
                "transfer destination",
                true,
                Box::new(|a| {
                    if let BridgePlanStep::Transfer { destination, .. } =
                        &mut a.graph_projection.nodes[1].step
                    {
                        *destination = TransferDestination::SelectedDevice {
                            device_ref: "selected".into(),
                        };
                    }
                }),
            ),
            (
                "revision hash",
                true,
                Box::new(|a| a.revision_hash = "sha256:wrong-revision".into()),
            ),
            (
                "graph hash",
                false,
                Box::new(|a| a.graph_projection.graph_hash = "sha256:wrong-graph".into()),
            ),
        ];

        for (index, (name, recompute_hash, change)) in cases.into_iter().enumerate() {
            let (_paths, store) = store();
            let revision = ready(&store);
            let approval_id = format!("semantic-mismatch-{index}");
            let attempt_id = format!("semantic-attempt-{index}");
            store
                .create_approval(&approval(&revision, &approval_id), 20)
                .unwrap();
            let mut candidate = attempt(&revision, &approval_id, &attempt_id);
            change(&mut candidate);
            if recompute_hash {
                candidate.graph_projection.graph_hash =
                    canonical_graph_hash(&candidate.graph_projection).unwrap();
            }
            assert!(
                store
                    .consume_approval_create_attempt(&candidate, 21)
                    .is_err(),
                "{name}"
            );
            let conn = connection(store.paths).unwrap();
            let state: String = conn
                .query_row(
                    "SELECT state FROM bridge_plan_approvals WHERE approval_id = ?1",
                    [&approval_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(state, "valid", "{name}");
            assert!(store.list_attempt(&attempt_id).is_err(), "{name}");
        }
    }

    #[test]
    fn phase2_concurrent_admission_creates_one_attempt_and_preserves_loser_state() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "concurrent-admission"), 20)
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for index in 0..2 {
            let paths = paths.clone();
            let barrier = barrier.clone();
            joins.push(std::thread::spawn(move || {
                let store = BridgePlanStore::new(Box::leak(Box::new(paths)));
                barrier.wait();
                store.create_attempt_from_approval(
                    &format!("concurrent-attempt-{index}"),
                    "concurrent-admission",
                    21,
                )
            }));
        }
        barrier.wait();
        let outcomes = joins
            .into_iter()
            .map(|join| join.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        let conn = connection(&paths).unwrap();
        let state: String = conn
            .query_row("SELECT state FROM bridge_plan_approvals WHERE approval_id = 'concurrent-admission'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "consumed");
        let records = (0..2)
            .filter_map(|index| {
                store
                    .list_attempt(&format!("concurrent-attempt-{index}"))
                    .ok()
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].steps.len(), revision.steps.len());
        assert!(records[0].steps.iter().all(|step| matches!(
            step.state,
            StepExecutionState::Eligible | StepExecutionState::Pending
        )));
    }

    #[test]
    fn phase2_transform_and_presentation_mismatches_leave_approval_unconsumed() {
        type RevisionMutation = Box<dyn Fn(&mut BridgePlanRevision)>;
        let cases: Vec<(&str, RevisionMutation)> = vec![
            (
                "transform intent",
                Box::new(|r| {
                    if let BridgePlanStep::Transform { intent, .. } = &mut r.steps[1] {
                        *intent = "Extract tables instead.".into();
                    }
                }),
            ),
            (
                "expected transform input",
                Box::new(|r| {
                    if let BridgePlanStep::Transform { expected_input, .. } = &mut r.steps[1] {
                        expected_input.object_type = "image".into();
                    }
                }),
            ),
            (
                "expected transform output",
                Box::new(|r| {
                    if let BridgePlanStep::Transform {
                        expected_output, ..
                    } = &mut r.steps[1]
                    {
                        expected_output.media_types = vec!["application/json".into()];
                    }
                }),
            ),
            (
                "presentation missing step",
                Box::new(|r| {
                    r.presentation
                        .step_explanations
                        .retain(|p| p.step_id != "transform");
                }),
            ),
            (
                "presentation extra step",
                Box::new(|r| {
                    r.presentation.step_explanations.push(StepExplanation {
                        step_id: "extra".into(),
                        action_summary: "Extra.".into(),
                        expected_result: "Extra.".into(),
                    })
                }),
            ),
            (
                "presentation wrong step",
                Box::new(|r| r.presentation.step_explanations[1].step_id = "transfer".into()),
            ),
            (
                "presentation duplicate mapping",
                Box::new(|r| r.presentation.step_explanations[2].step_id = "transform".into()),
            ),
        ];
        for (index, (name, mutate)) in cases.into_iter().enumerate() {
            let (_paths, store) = store();
            let revision = transform_revision();
            store.create_plan(&plan(), BridgePlanState::Draft).unwrap();
            store
                .append_revision(&revision, RevisionState::Proposed, 11)
                .unwrap();
            store
                .transition_plan("plan", BridgePlanState::Open)
                .unwrap();
            store
                .transition_revision("revision", RevisionState::Available)
                .unwrap();
            let approval_id = format!("transform-mismatch-{index}");
            store
                .create_approval(&approval(&revision, &approval_id), 20)
                .unwrap();
            let mut altered = revision.clone();
            mutate(&mut altered);
            altered.revision_hash = if name.starts_with("presentation") {
                "sha256:presentation-mismatch".into()
            } else {
                canonical_revision_hash(&altered).unwrap()
            };
            let mut candidate = attempt(
                &revision,
                &approval_id,
                &format!("transform-attempt-{index}"),
            );
            candidate.revision_hash = altered.revision_hash;
            assert!(
                store
                    .consume_approval_create_attempt(&candidate, 21)
                    .is_err(),
                "{name}"
            );
            let conn = connection(store.paths).unwrap();
            let state: String = conn
                .query_row(
                    "SELECT state FROM bridge_plan_approvals WHERE approval_id = ?1",
                    [&approval_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(state, "valid", "{name}");
        }
    }

    #[test]
    fn phase2_authority_rejects_lifecycle_and_complete_binding_mismatches() {
        let (_paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "authmatrix"), 20)
            .unwrap();
        store
            .create_attempt_from_approval("authmatrix-attempt", "authmatrix", 21)
            .unwrap();
        let authorities = EphemeralStepAuthorityStore::default();
        let id = authorities
            .derive(&store, "authmatrix-attempt", "search", 22, 30)
            .unwrap();
        assert!(authorities.consume(&store, &id, 30).is_err());
        authorities
            .transition_step(
                &store,
                "authmatrix-attempt",
                "search",
                StepExecutionState::Running,
                23,
            )
            .unwrap();
        assert!(authorities.consume(&store, &id, 24).is_err());
        let fresh = EphemeralStepAuthorityStore::default();
        assert!(fresh.consume(&store, &id, 24).is_err());
    }

    #[test]
    fn phase2_dependency_eligibility_matrix_requires_all_completed_predecessors() {
        let (_paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "deps"), 20)
            .unwrap();
        store
            .create_attempt_from_approval("deps-attempt", "deps", 21)
            .unwrap();
        let base = store.list_attempt("deps-attempt").unwrap();
        let transfer = base.attempt.graph_projection.nodes[1].clone();
        assert!(
            ensure_step_dependencies_completed(&base, "search").is_ok(),
            "no dependencies"
        );
        for state in [
            StepExecutionState::Pending,
            StepExecutionState::Eligible,
            StepExecutionState::Authorized,
            StepExecutionState::Running,
            StepExecutionState::Failed,
            StepExecutionState::Cancelled,
        ] {
            let mut record = base.clone();
            record
                .steps
                .iter_mut()
                .find(|s| s.step_id == "search")
                .unwrap()
                .state = state;
            assert!(ensure_step_dependencies_completed(&record, "transfer").is_err());
        }
        let mut completed = base.clone();
        completed
            .steps
            .iter_mut()
            .find(|s| s.step_id == "search")
            .unwrap()
            .state = StepExecutionState::Completed;
        assert!(ensure_step_dependencies_completed(&completed, "transfer").is_ok());
        let mut multi = completed.clone();
        let mut extra = transfer.clone();
        extra.node_id = "node-extra".into();
        extra.step_id = "extra".into();
        multi.attempt.graph_projection.nodes.push(extra);
        multi.attempt.graph_projection.nodes[1]
            .depends_on_node_ids
            .push("node-extra".into());
        multi.steps.push(StepExecutionProjection {
            attempt_id: "deps-attempt".into(),
            step_id: "extra".into(),
            state: StepExecutionState::Completed,
            updated_at: 22,
        });
        assert!(
            ensure_step_dependencies_completed(&multi, "transfer").is_ok(),
            "multiple completed"
        );
        multi.steps.last_mut().unwrap().state = StepExecutionState::Failed;
        assert!(
            ensure_step_dependencies_completed(&multi, "transfer").is_err(),
            "mixed completed and failed"
        );
        multi.steps.last_mut().unwrap().state = StepExecutionState::Cancelled;
        assert!(
            ensure_step_dependencies_completed(&multi, "transfer").is_err(),
            "mixed completed and cancelled"
        );
        // CAS rejects a stale concurrent eligibility decision after the predecessor changed.
        store
            .transition_step("deps-attempt", "search", StepExecutionState::Authorized, 23)
            .unwrap();
        assert!(store
            .transition_step("deps-attempt", "transfer", StepExecutionState::Eligible, 24)
            .is_err());
    }

    #[test]
    fn phase2_authority_binding_matrix_rejects_each_tampered_field() {
        type Mutation = Box<dyn Fn(&mut EphemeralStepAuthority)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("bridge", Box::new(|a| a.bridge_id = "other".into())),
            ("plan", Box::new(|a| a.plan_id = "other".into())),
            ("revision", Box::new(|a| a.revision_id = "other".into())),
            (
                "revision hash",
                Box::new(|a| a.revision_hash = "sha256:other".into()),
            ),
            ("approval", Box::new(|a| a.approval_id = "other".into())),
            ("attempt", Box::new(|a| a.attempt_id = "other".into())),
            ("step", Box::new(|a| a.step_id = "transfer".into())),
            (
                "operation",
                Box::new(|a| a.operation = StepOperation::Transfer),
            ),
            (
                "source device",
                Box::new(|a| a.source_device_ref = Some("requester".into())),
            ),
            (
                "execution device",
                Box::new(|a| a.execution_device_ref = "requester".into()),
            ),
            (
                "input slots",
                Box::new(|a| a.input_slot_ids.push("other".into())),
            ),
            (
                "output slots",
                Box::new(|a| a.output_slot_ids.push("other".into())),
            ),
            (
                "selection constraints",
                Box::new(|a| a.object_selection_digest = "sha256:other".into()),
            ),
            (
                "transform contract",
                Box::new(|a| a.transform_contract_digest = "sha256:other".into()),
            ),
            (
                "transfer destination",
                Box::new(|a| a.transfer_destination_digest = "sha256:other".into()),
            ),
        ];
        for (index, (name, mutate)) in cases.into_iter().enumerate() {
            let (_paths, store) = store();
            let revision = ready(&store);
            let approval_id = format!("bind-{index}");
            let attempt_id = format!("bindattempt-{index}");
            store
                .create_approval(&approval(&revision, &approval_id), 20)
                .unwrap();
            store
                .create_attempt_from_approval(&attempt_id, &approval_id, 21)
                .unwrap();
            let authorities = EphemeralStepAuthorityStore::default();
            let id = authorities
                .derive(&store, &attempt_id, "search", 22, 40)
                .unwrap();
            mutate(authorities.grants.lock().unwrap().get_mut(&id).unwrap());
            assert!(authorities.consume(&store, &id, 23).is_err(), "{name}");
        }
    }

    #[test]
    fn phase2_authority_rejects_every_terminal_step_and_attempt_state() {
        for terminal in [
            StepExecutionState::Completed,
            StepExecutionState::Failed,
            StepExecutionState::Cancelled,
        ] {
            let (_paths, store) = store();
            let revision = ready(&store);
            let approval_id = format!("stepterminal-{}", terminal.as_str());
            let attempt_id = format!("stepattempt-{}", terminal.as_str());
            store
                .create_approval(&approval(&revision, &approval_id), 20)
                .unwrap();
            store
                .create_attempt_from_approval(&attempt_id, &approval_id, 21)
                .unwrap();
            let authorities = EphemeralStepAuthorityStore::default();
            let id = authorities
                .derive(&store, &attempt_id, "search", 22, 40)
                .unwrap();
            if terminal != StepExecutionState::Cancelled {
                authorities
                    .transition_step(
                        &store,
                        &attempt_id,
                        "search",
                        StepExecutionState::Running,
                        23,
                    )
                    .unwrap();
            }
            authorities
                .transition_step(&store, &attempt_id, "search", terminal, 24)
                .unwrap();
            assert!(authorities.consume(&store, &id, 25).is_err());
        }
        for terminal in [
            AttemptState::Interrupted,
            AttemptState::Completed,
            AttemptState::Failed,
            AttemptState::Cancelled,
        ] {
            let (_paths, store) = store();
            let revision = ready(&store);
            let approval_id = format!("attemptterminal-{}", terminal.as_str());
            let attempt_id = format!("attemptstate-{}", terminal.as_str());
            store
                .create_approval(&approval(&revision, &approval_id), 20)
                .unwrap();
            store
                .create_attempt_from_approval(&attempt_id, &approval_id, 21)
                .unwrap();
            let authorities = EphemeralStepAuthorityStore::default();
            let id = authorities
                .derive(&store, &attempt_id, "search", 22, 40)
                .unwrap();
            if terminal != AttemptState::Interrupted {
                authorities
                    .transition_attempt(&store, &attempt_id, AttemptState::Running, 23)
                    .unwrap();
            }
            authorities
                .transition_attempt(&store, &attempt_id, terminal, 24)
                .unwrap();
            assert!(authorities.consume(&store, &id, 25).is_err());
        }
    }

    #[test]
    fn phase2_expiry_and_burn_admission_ordering_are_fail_closed() {
        let (paths, primary_store) = store();
        let revision = ready(&primary_store);
        let mut expired = approval(&revision, "expirywins");
        expired.expires_at = 21;
        primary_store.create_approval(&expired, 20).unwrap();
        assert!(primary_store
            .create_attempt_from_approval("expiry-attempt", "expirywins", 21)
            .is_err());
        let conn = connection(&paths).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM bridge_plan_approvals WHERE approval_id = 'expirywins'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "expired");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM bridge_plan_attempts WHERE approval_id = 'expirywins'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM bridge_plan_attempt_steps", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        let admitted = approval(&revision, "admissionwins");
        primary_store.create_approval(&admitted, 20).unwrap();
        primary_store
            .create_attempt_from_approval("admission-attempt", "admissionwins", 20)
            .unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM bridge_plan_approvals WHERE approval_id = 'admissionwins'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "consumed");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM bridge_plan_attempt_steps WHERE attempt_id = 'admission-attempt'", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        let (_, burned_store) = self::store();
        let burned_revision = ready(&burned_store);
        burned_store
            .create_approval(&approval(&burned_revision, "burnwins"), 20)
            .unwrap();
        let burned_conn = connection(burned_store.paths).unwrap();
        burned_conn
            .execute(
                "INSERT INTO burned_bridges (room_id, burned_at) VALUES ('bridge', 21)",
                [],
            )
            .unwrap();
        assert!(burned_store
            .create_attempt_from_approval("burn-attempt", "burnwins", 21)
            .is_err());
        assert_eq!(
            burned_conn
                .query_row(
                    "SELECT state FROM bridge_plan_approvals WHERE approval_id = 'burnwins'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "valid"
        );
        assert_eq!(
            burned_conn
                .query_row("SELECT COUNT(*) FROM bridge_plan_attempts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn phase2_raw_sql_immutability_boundary_matrix() {
        let (paths, store) = store();
        let revision = ready(&store);
        store
            .create_approval(&approval(&revision, "sqlmatrix"), 20)
            .unwrap();
        store
            .create_attempt_from_approval("sqlmatrix-attempt", "sqlmatrix", 21)
            .unwrap();
        let conn = connection(&paths).unwrap();
        assert!(conn.execute("UPDATE bridge_plan_attempts SET attempt_json = '{}' WHERE attempt_id = 'sqlmatrix-attempt'", []).is_err());
        assert!(conn.execute("UPDATE bridge_plan_attempts SET revision_id = 'other' WHERE attempt_id = 'sqlmatrix-attempt'", []).is_err());
        assert!(conn
            .execute(
                "DELETE FROM bridge_plan_revisions WHERE revision_id = 'revision'",
                []
            )
            .is_err());
        assert!(conn.execute("UPDATE bridge_plan_revisions SET revision_json = '{}' WHERE revision_id = 'revision'", []).is_err());
        assert!(conn
            .execute("DELETE FROM bridge_plans WHERE plan_id = 'plan'", [])
            .is_err());
        conn.execute(
            "INSERT INTO burned_bridges (room_id, burned_at) VALUES ('bridge', 22)",
            [],
        )
        .unwrap();
        assert!(conn.execute("INSERT INTO bridge_plan_attempt_steps (attempt_id, step_id, state, updated_at) VALUES ('sqlmatrix-attempt', 'new', 'pending', 23)", []).is_err());
        assert!(conn.execute("UPDATE bridge_plan_attempt_steps SET state = 'cancelled' WHERE attempt_id = 'sqlmatrix-attempt' AND step_id = 'search'", []).is_err());
    }

    #[test]
    fn file_search_transfer_revision_binds_selection_and_requester_destination() {
        let revision = build_file_plan_revision(
            "bridge".into(),
            "requester".into(),
            "selected".into(),
            "Find the report and bring it here.".into(),
            "report".into(),
            vec!["pdf".into()],
            vec!["documents".into()],
            true,
        )
        .unwrap();
        assert_eq!(
            revision.search_selection_mode,
            SearchSelectionMode::BoundedInline
        );
        assert_eq!(revision.steps.len(), 2);
        let BridgePlanStep::Search {
            selection: Some(selection),
            ..
        } = &revision.steps[0]
        else {
            panic!("file transfer plan must select one bounded Search result");
        };
        assert_eq!(selection.source_slot_id, "found");
        assert_eq!(selection.downstream_slot_id, "selected_file");
        let BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            source,
            destination,
            ..
        } = &revision.steps[1]
        else {
            panic!("file transfer plan must include Transfer");
        };
        assert_eq!(depends_on, &vec!["search"]);
        assert_eq!(input_slots[0].slot_id, "selected_file");
        assert_eq!(
            source,
            &ObjectSelectionRule::FromSlot {
                slot_id: "selected_file".into()
            }
        );
        assert_eq!(
            destination,
            &TransferDestination::RequestingDevice {
                device_ref: "requester".into()
            }
        );
        assert!(validate_revision(&revision).is_ok());
    }

    #[test]
    fn file_transform_transfer_revision_binds_generated_output_to_transfer() {
        let revision = build_file_transform_revision(
            "bridge".into(),
            "requester".into(),
            "selected".into(),
            "Extract the readable text from my report and bring it here.".into(),
            "report".into(),
            vec!["txt".into()],
            vec!["documents".into()],
            "extract readable text".into(),
            true,
        )
        .unwrap();
        assert_eq!(revision.steps.len(), 3);
        let BridgePlanStep::Transfer {
            depends_on,
            input_slots,
            source,
            destination,
            ..
        } = &revision.steps[2]
        else {
            panic!("transform plan must end with Transfer");
        };
        assert_eq!(depends_on, &vec!["transform"]);
        assert_eq!(input_slots[0].slot_id, "transformed_file");
        assert_eq!(
            source,
            &ObjectSelectionRule::FromSlot {
                slot_id: "transformed_file".into()
            }
        );
        assert_eq!(
            destination,
            &TransferDestination::RequestingDevice {
                device_ref: "requester".into()
            }
        );
        assert!(validate_revision(&revision).is_ok());
    }

    #[test]
    fn direct_transfer_revision_binds_requester_selection_to_selected_device() {
        let revision = build_direct_file_transfer_revision(
            "bridge".into(),
            "requester".into(),
            "selected".into(),
            "Send one local document to the selected device.".into(),
        )
        .unwrap();
        assert_eq!(revision.steps.len(), 1);
        let BridgePlanStep::Transfer {
            depends_on,
            source_device_ref,
            execution_device_ref,
            source,
            destination,
            ..
        } = &revision.steps[0]
        else {
            panic!("direct plan must contain Transfer");
        };
        assert!(depends_on.is_empty());
        assert_eq!(source_device_ref.as_deref(), Some("requester"));
        assert_eq!(execution_device_ref, "requester");
        assert!(matches!(source, ObjectSelectionRule::FutureUserSelection { .. }));
        assert_eq!(
            destination,
            &TransferDestination::SelectedDevice {
                device_ref: "selected".into()
            }
        );
        assert!(validate_revision(&revision).is_ok());
    }
}

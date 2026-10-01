//! Bounded data over Room Control. Correlation never conveys executable authority.
use super::{contracts::*, evidence::*, require, values::*};
use crate::{error::AppResult, host_identity::HostRef};
use serde::{Deserialize, Serialize};
pub(crate) const PROTOCOL: &str = "physical-control-v2";
pub(crate) const MAX_BYTES: usize = 48 * 1024;

claim!(PhysicalMessageV1 {
    protocol: String,
    semantic_id: RequestId,
    session_pair: String,
    requester: HostRef,
    executor: HostRef,
    operation: PhysicalOperationV1
});
impl PhysicalMessageV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        require(
            self.protocol == PROTOCOL
                && self.session_pair.len() <= 160
                && !self.session_pair.is_empty(),
            "Incompatible physical protocol/session",
        )?;
        validate_host(&self.requester)?;
        validate_host(&self.executor)?;
        require(
            self.requester != self.executor,
            "Physical remote Hosts must differ",
        )?;
        self.operation.validate()?;
        require(
            serde_json::to_vec(self)?.len() <= MAX_BYTES,
            "Oversized physical message",
        )
    }
    pub(crate) fn digest(&self) -> AppResult<DigestV1> {
        self.validate()?;
        digest("pastey-physical-semantic-v1", self)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PhysicalOperationV1 {
    Discover,
    Environments {
        offers: Vec<PhysicalReviewScopeV1>,
    },
    Start {
        review: PhysicalReviewRecordV1,
    },
    Cancel {
        start: RequestId,
    },
    StatusQuery {
        start: RequestId,
    },
    Reconcile {
        start: RequestId,
    },
    Status {
        start: RequestId,
        status: PhysicalStatusV1,
    },
    /// Decision-stream tools, forwarded to the executor's dispatcher. The
    /// requester only relays; admission happens on the executor.
    ToolOpen {
        start: RequestId,
        caller: LabelV1,
    },
    ToolCall {
        start: RequestId,
        tool_session: RequestId,
        call: super::core::DecisionToolCallV1,
    },
    ToolClose {
        start: RequestId,
        tool_session: RequestId,
    },
    ToolResult {
        request: RequestId,
        outcome: ToolOutcomeV1,
    },
}
/// The executor's answer to one forwarded tool request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ToolOutcomeV1 {
    Opened {
        tool_session: RequestId,
        tools: Vec<String>,
    },
    Reply {
        reply: super::core::DecisionToolReplyV1,
    },
    /// The tool session closed and ended the stream (its brain had
    /// committed, or the stream had already ended).
    Closed,
    /// The tool session closed before it committed: only its reservation
    /// was released, and the stream is unaffected.
    Released,
    Failed {
        reason: String,
    },
}
impl PhysicalOperationV1 {
    fn validate(&self) -> AppResult<()> {
        match self {
            Self::Environments { offers } => {
                require(offers.len() <= 8, "Too many physical environments")?;
                for s in offers {
                    s.fields().validate()?;
                }
            }
            Self::Start { review } => {
                review.validate()?;
                require(
                    review.state == PhysicalReviewStateV1::Approved && review.approval.is_some(),
                    "Remote Start lacks exact approval",
                )?;
            }
            Self::Status { status, .. } => status.validate()?,
            Self::ToolResult {
                outcome: ToolOutcomeV1::Opened { tools, .. },
                ..
            } => require(tools.len() <= 40, "Too many tools")?,
            _ => {}
        }
        Ok(())
    }
    pub(crate) fn is_response(&self) -> bool {
        matches!(
            self,
            Self::Environments { .. } | Self::Status { .. } | Self::ToolResult { .. }
        )
    }
}
/// Decision records a status carries: the most recent ones.
pub(crate) const STATUS_DECISIONS: usize = 32;
const MAX_DECISION_TEXT: usize = 256;
/// Cuts recorded caller text to what a status carries, at a character
/// boundary.
pub(crate) fn status_text(mut text: String) -> String {
    if text.len() > MAX_DECISION_TEXT {
        let mut end = MAX_DECISION_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}
// One decision record as Review shows it: who proposed which option, and
// Core's admission (allowed, or refused with the reason). A refused
// proposal keeps the caller's text as recorded.
claim!(DecisionSummaryV1 {
    sequence: u64,
    proposer: String,
    option: String,
    allowed: bool,
    reason: String,
});
impl DecisionSummaryV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        require(
            self.proposer.len() <= MAX_DECISION_TEXT
                && self.option.len() <= MAX_DECISION_TEXT
                && self.reason.len() <= MAX_DECISION_TEXT,
            "Oversized decision record",
        )
    }
}
// The witness's latest verdict on the completion contract. Core's own
// conclusion is the status's `consequence`.
claim!(WitnessConclusionV1 {
    witness_class: WitnessClassV1,
    result: WitnessResultV1,
    reason: LabelV1,
});
impl WitnessConclusionV1 {
    pub(crate) fn validate(&self) -> AppResult<()> {
        Ok(())
    }
}
claim!(PhysicalStatusV1 {
    review: PhysicalReviewStateV1,
    authority: PhysicalAuthorityStateV1,
    installation: PhysicalInstallationStateV1,
    dispatch: PhysicalDispatchStateV1,
    consequence: ConsequenceStateV1,
    acceptance: AcceptanceStateV1,
    reconciliation: PhysicalReconciliationStateV1,
    enforcement_pending: bool,
    quarantined: bool,
    root: Option<RootId>,
    session: Option<SessionId>,
    action: Option<ActionId>,
    /// Why Core concluded `consequence` for the latest action.
    consequence_reason: Option<LabelV1>,
    witness: Option<WitnessConclusionV1>,
    decisions: Vec<DecisionSummaryV1>
});
impl PhysicalStatusV1 {
    pub(in crate::physical) fn validate(&self) -> AppResult<()> {
        require(
            self.acceptance != AcceptanceStateV1::Accepted
                || self.consequence == ConsequenceStateV1::Verified,
            "Task acceptance lacks verified consequence",
        )?;
        require(
            self.decisions.len() <= STATUS_DECISIONS,
            "Too many decision records",
        )
    }
    pub(crate) fn pending() -> Self {
        Self {
            review: PhysicalReviewStateV1::Approved,
            authority: PhysicalAuthorityStateV1::Pending,
            installation: PhysicalInstallationStateV1::Pending,
            dispatch: PhysicalDispatchStateV1::Unknown,
            consequence: ConsequenceStateV1::OutcomeUnknown,
            acceptance: AcceptanceStateV1::Pending,
            reconciliation: PhysicalReconciliationStateV1::Pending,
            enforcement_pending: true,
            quarantined: true,
            root: None,
            session: None,
            action: None,
            consequence_reason: None,
            witness: None,
            decisions: Vec::new(),
        }
    }
}
macro_rules! states {
    ($name:ident { $($v:ident),* }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all="snake_case")]
        pub(crate) enum $name { $($v),* }
    }
}
states!(PhysicalAuthorityStateV1 {
    Pending,
    Open,
    Closed
});
states!(PhysicalInstallationStateV1 {
    Pending,
    Active,
    Quarantined
});
states!(PhysicalDispatchStateV1 {
    NotSent,
    IntentCommitted,
    Acknowledged,
    Refused,
    Unknown
});
states!(PhysicalReconciliationStateV1 { Pending, Recorded });
